//! Le SDK NDI pour Linux : téléchargé et déballé dans un dossier temporaire, sa licence lue ; rien
//! n'est installé tant qu'elle n'est pas acceptée (l'étape « Bibliothèque NDI » le fait ensuite).
//!
//! Le SDK arrive en `.tar.gz` contenant un installateur `.sh` : la licence, une question, puis une
//! archive collée à la fin du script. Le script n'est pas lancé (il demande `bash`, et accepterait
//! la licence à la place de l'utilisateur) : sa licence et son archive en sont lues directement.
use std::{
    io::Read,
    path::{Path, PathBuf},
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

/// Ce que contient l'installateur `.sh` : le texte de sa licence, et l'archive `.tar.gz` collée
/// après la ligne `__NDI_ARCHIVE_BEGIN__`.
pub fn split_installer(script: &[u8]) -> Result<(String, &[u8]), String> {
    let marker = b"\n__NDI_ARCHIVE_BEGIN__\n";
    let at = script.windows(marker.len()).position(|w| w == marker).ok_or("the NDI SDK installer holds no archive")?;
    let head = String::from_utf8_lossy(&script[..at]);
    // La licence : entre « << NDI_EULA_END » et la ligne « NDI_EULA_END ».
    let license = head
        .split_once("<< NDI_EULA_END\n")
        .and_then(|(_, rest)| rest.split_once("\nNDI_EULA_END"))
        .map(|(eula, _)| eula.trim().to_string())
        .unwrap_or_default();
    Ok((license, &script[at + marker.len()..]))
}

/// Télécharge et déballe le SDK dans `dir`.
pub fn fetch(dir: &Path) -> Result<Sdk, String> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(300))).build().into();
    let res = agent.get(SDK_URL).call().map_err(|e| format!("cannot download the NDI SDK: {e}"))?;
    let tgz = dir.join("ndi-sdk.tar.gz");
    let mut f = std::fs::File::create(&tgz).map_err(|e| e.to_string())?;
    std::io::copy(&mut res.into_body().into_reader(), &mut f).map_err(|e| format!("the download was cut: {e}"))?;
    unpack(&tgz, dir)
}

/// Déballe le SDK téléchargé (`.tar.gz` de l'installateur `.sh`, ou le `.sh` seul) dans `dir`.
pub fn unpack(download: &Path, dir: &Path) -> Result<Sdk, String> {
    let script = if download.to_string_lossy().ends_with(".sh") {
        std::fs::read(download).map_err(|e| e.to_string())?
    } else {
        let f = std::fs::File::open(download).map_err(|e| e.to_string())?;
        let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(f));
        let mut found = None;
        for e in ar.entries().map_err(|e| format!("cannot unpack the NDI SDK: {e}"))? {
            let mut e = e.map_err(|e| format!("cannot unpack the NDI SDK: {e}"))?;
            if e.path().is_ok_and(|p| p.extension().is_some_and(|x| x == "sh")) {
                let mut v = Vec::new();
                e.read_to_end(&mut v).map_err(|e| e.to_string())?;
                found = Some(v);
                break;
            }
        }
        found.ok_or("no installer in the NDI SDK")?
    };
    let (eula, archive) = split_installer(&script)?;
    tar::Archive::new(flate2::read::GzDecoder::new(archive)).unpack(dir).map_err(|e| format!("cannot unpack the NDI SDK: {e}"))?;
    let sdk = dir.join("NDI SDK for Linux");
    let lib = lib_in(&sdk).ok_or("no NDI library for x86-64 in the SDK")?;
    Ok(Sdk { lib, license: if eula.is_empty() { license_in(&sdk) } else { eula } })
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

    #[test]
    fn reads_the_installer_without_running_it() {
        // Un installateur comme celui de NDI : licence, question, archive collée à la fin.
        let base = std::env::temp_dir().join(format!("strata-ndi-sh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("src/NDI SDK for Linux/lib/x86_64-linux-gnu");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("libndi.so.6.4.0"), b"elf").unwrap();
        let mut tgz = Vec::new();
        {
            let gz = flate2::write::GzEncoder::new(&mut tgz, flate2::Compression::fast());
            let mut t = tar::Builder::new(gz);
            t.append_dir_all("NDI SDK for Linux", base.join("src/NDI SDK for Linux")).unwrap();
            t.into_inner().unwrap().finish().unwrap();
        }
        let mut script = b"#!/bin/sh\n$view_eula << NDI_EULA_END\nNDI SDK License Agreement\nTerms.\nNDI_EULA_END\n\nread -p \"Type y\" REPLY\nexit 0\n__NDI_ARCHIVE_BEGIN__\n".to_vec();
        script.extend_from_slice(&tgz);
        let sh = base.join("Install_NDI_SDK_v6_Linux.sh");
        std::fs::write(&sh, &script).unwrap();
        let sdk = unpack(&sh, &base.join("out")).unwrap();
        assert_eq!(sdk.license, "NDI SDK License Agreement\nTerms.");
        assert_eq!(sdk.lib.file_name().unwrap(), "libndi.so.6.4.0");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Avec le vrai SDK téléchargé : `STRATA_NDI_SDK=…/Install_NDI_SDK_v6_Linux.tar.gz cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn reads_the_real_sdk() {
        let tgz = PathBuf::from(std::env::var("STRATA_NDI_SDK").unwrap());
        let out = std::env::temp_dir().join(format!("strata-ndi-real-{}", std::process::id()));
        let sdk = unpack(&tgz, &out).unwrap();
        assert!(sdk.license.starts_with("NDI SDK License Agreement"), "{}", &sdk.license[..80]);
        println!("{} · licence {} caractères", sdk.lib.display(), sdk.license.len());
        let _ = std::fs::remove_dir_all(&out);
    }
}
