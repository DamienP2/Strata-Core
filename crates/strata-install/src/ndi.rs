//! Le SDK NDI pour Linux : téléchargé et déballé dans un dossier temporaire, sa licence lue ; rien
//! n'est installé tant qu'elle n'est pas acceptée (l'étape « Bibliothèque NDI » le fait ensuite).
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

/// Le SDK NDI pour Linux (ndi.video).
pub const SDK_URL: &str = "https://downloads.ndi.tv/SDK/NDI_SDK_Linux/Install_NDI_SDK_v6_Linux.tar.gz";

/// Le SDK déballé : la bibliothèque pour ce processeur, et le texte de la licence.
pub struct Sdk {
    pub lib: PathBuf,
    pub license: String,
}

/// La bibliothèque d'un dossier « NDI SDK for Linux » (le fichier, pas un lien).
pub fn lib_in(sdk: &Path) -> Option<PathBuf> {
    let dir = sdk.join("lib/x86_64-linux-gnu");
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("libndi.so")) && p.is_file() && !p.is_symlink())
        .collect();
    v.sort();
    v.pop()
}

/// Le texte de la licence (le premier fichier qui en a l'air).
pub fn license_in(sdk: &Path) -> String {
    let mut files: Vec<PathBuf> = std::fs::read_dir(sdk)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name().is_some_and(|n| {
                    let n = n.to_string_lossy().to_lowercase();
                    n.contains("licen") || n.contains("eula")
                })
        })
        .collect();
    files.sort();
    files
        .first()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .unwrap_or_else(|| "NDI SDK License Agreement: see https://ndi.video/legal/ (the SDK holds no license file)".into())
}

/// Télécharge et déballe le SDK dans `dir`.
pub fn fetch(dir: &Path) -> Result<Sdk, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(300))).build().into();
    let res = agent.get(SDK_URL).call().map_err(|e| format!("cannot download the NDI SDK: {e}"))?;
    let tgz = dir.join("ndi-sdk.tar.gz");
    let mut f = std::fs::File::create(&tgz).map_err(|e| e.to_string())?;
    std::io::copy(&mut res.into_body().into_reader(), &mut f).map_err(|e| format!("the download was cut: {e}"))?;
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(std::fs::File::open(&tgz).map_err(|e| e.to_string())?));
    ar.unpack(dir).map_err(|e| format!("cannot unpack the NDI SDK: {e}"))?;
    let script = std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "sh"))
        .ok_or("no installer in the NDI SDK")?;
    // Son installateur ne fait que déballer « NDI SDK for Linux » ici ; la licence est montrée
    // ensuite, et rien n'est installé avant qu'elle soit acceptée.
    let ok = Command::new("sh")
        .arg("-c")
        .arg(format!("yes | PAGER=cat sh '{}' >/dev/null 2>&1", script.display()))
        .current_dir(dir)
        .status()
        .map_err(|e| e.to_string())?;
    let sdk = dir.join("NDI SDK for Linux");
    if !ok.success() && !sdk.exists() {
        return Err("the NDI SDK installer failed".into());
    }
    let lib = lib_in(&sdk).ok_or("no NDI library for x86-64 in the SDK")?;
    Ok(Sdk { lib, license: license_in(&sdk) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_library_and_the_license() {
        let dir = std::env::temp_dir().join(format!("strata-ndi-{}/NDI SDK for Linux", std::process::id()));
        std::fs::create_dir_all(dir.join("lib/x86_64-linux-gnu")).unwrap();
        std::fs::write(dir.join("lib/x86_64-linux-gnu/libndi.so.6.1.1"), b"elf").unwrap();
        std::os::unix::fs::symlink("libndi.so.6.1.1", dir.join("lib/x86_64-linux-gnu/libndi.so.6")).unwrap();
        std::fs::write(dir.join("NDI SDK License Agreement.txt"), "THE LICENSE").unwrap();
        assert_eq!(lib_in(&dir).unwrap().file_name().unwrap(), "libndi.so.6.1.1");
        assert_eq!(license_in(&dir), "THE LICENSE");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }
}
