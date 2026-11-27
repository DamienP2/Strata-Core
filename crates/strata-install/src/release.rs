//! Le Core pris dans les releases GitHub de Strata-Core (sa fiche `release.json`, son empreinte),
//! ou dans une archive donnée (serveur sans internet).
use std::{
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Deserialize)]
struct Manifest {
    version: String,
    assets: Vec<ManifestAsset>,
}

#[derive(Deserialize)]
struct ManifestAsset {
    name: String,
    kind: String,
    sha256: String,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .user_agent(format!("strata-install/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn sha256_file(p: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(p).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// La dernière release (ou `vX.Y.Z`) : télécharge l'archive du Core dans `dir`, vérifie son
/// empreinte ; (version, archive).
pub fn fetch_core(repo: &str, version: Option<&str>, dir: &Path, progress: &dyn Fn(u64, u64)) -> Result<(String, PathBuf), String> {
    let a = agent();
    let url = match version {
        Some(v) => format!("https://api.github.com/repos/{repo}/releases/tags/v{}", v.trim_start_matches('v')),
        None => format!("https://api.github.com/repos/{repo}/releases/latest"),
    };
    let json = |r: ureq::http::Response<ureq::Body>| -> Result<String, String> { r.into_body().read_to_string().map_err(|e| e.to_string()) };
    let rel: GhRelease =
        serde_json::from_str(&json(a.get(&url).header("accept", "application/vnd.github+json").call().map_err(|e| format!("cannot reach GitHub: {e}"))?)?)
            .map_err(|e| e.to_string())?;
    let manifest_url = rel.assets.iter().find(|x| x.name == "release.json").ok_or_else(|| format!("release {} has no release.json", rel.tag_name))?;
    let m: Manifest = serde_json::from_str(&json(a.get(&manifest_url.browser_download_url).call().map_err(|e| e.to_string())?)?).map_err(|e| e.to_string())?;
    let core = m.assets.iter().find(|x| x.kind == "core-linux-x86_64").ok_or_else(|| format!("release {} has no Core archive", rel.tag_name))?;
    let gh = rel.assets.iter().find(|x| x.name == core.name).ok_or_else(|| format!("{} is missing from the release", core.name))?;
    let res = a.get(&gh.browser_download_url).call().map_err(|e| format!("download: {e}"))?;
    let total = res.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0);
    let path = dir.join(&core.name);
    let mut out = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let mut body = res.into_body().into_reader();
    let mut buf = vec![0u8; 1 << 16];
    let mut done = 0u64;
    loop {
        let n = body.read(&mut buf).map_err(|e| format!("the download was cut: {e}"))?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut out, &buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        progress(done, total);
    }
    let sum = sha256_file(&path)?;
    if !sum.eq_ignore_ascii_case(core.sha256.trim()) {
        return Err("the download does not match its fingerprint".into());
    }
    Ok((m.version, path))
}

/// Sort le programme `strata-core` de son archive (`strata-core-<v>-linux-x86_64/strata-core`).
pub fn extract_core(archive: &Path, to: &Path) -> Result<(), String> {
    let f = std::fs::File::open(archive).map_err(|e| format!("cannot open {}: {e}", archive.display()))?;
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(f));
    for e in ar.entries().map_err(|_| "not a .tar.gz archive".to_string())? {
        let mut e = e.map_err(|_| "the archive is damaged".to_string())?;
        let p = e.path().map_err(|e| e.to_string())?.to_path_buf();
        if p.file_name().is_some_and(|n| n == "strata-core") && e.header().entry_type().is_file() {
            let mut out = std::fs::File::create(to).map_err(|e| format!("cannot write {}: {e}", to.display()))?;
            std::io::copy(&mut e, &mut out).map_err(|e| e.to_string())?;
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o755));
            return Ok(());
        }
    }
    Err("this archive holds no strata-core".into())
}

/// Sort les bibliothèques embarquées avec le Core (`lib/libmxl.so*` : MXL) dans `dir`. Rend
/// combien.
pub fn extract_libs(archive: &Path, dir: &Path) -> Result<usize, String> {
    let f = std::fs::File::open(archive).map_err(|e| format!("cannot open {}: {e}", archive.display()))?;
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(f));
    let mut n = 0;
    for e in ar.entries().map_err(|_| "not a .tar.gz archive".to_string())? {
        let mut e = e.map_err(|_| "the archive is damaged".to_string())?;
        let p = e.path().map_err(|e| e.to_string())?.to_path_buf();
        let in_lib = p.parent().and_then(|d| d.file_name()).is_some_and(|d| d == "lib");
        let Some(name) = p.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
        if in_lib && (name.starts_with("libmxl") || name.starts_with("LICENSE")) {
            std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            let to = dir.join(&name);
            let _ = std::fs::remove_file(&to);
            e.unpack(&to).map_err(|e| format!("cannot write {}: {e}", to.display()))?;
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_the_core_out_of_its_archive() {
        let dir = std::env::temp_dir().join(format!("strata-install-rel-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("pkg/strata-core-9.9.9-linux-x86_64")).unwrap();
        std::fs::write(dir.join("pkg/strata-core-9.9.9-linux-x86_64/strata-core"), b"#!/bin/sh\necho strata-core 9.9.9\n").unwrap();
        let status = std::process::Command::new("tar")
            .args(["-C", dir.join("pkg").to_str().unwrap(), "-czf", dir.join("core.tar.gz").to_str().unwrap(), "strata-core-9.9.9-linux-x86_64"])
            .status()
            .unwrap();
        assert!(status.success());
        extract_core(&dir.join("core.tar.gz"), &dir.join("bin")).unwrap();
        assert!(std::fs::read_to_string(dir.join("bin")).unwrap().contains("9.9.9"));
        assert_eq!(sha256_file(&dir.join("bin")).unwrap().len(), 64);
        assert!(extract_core(&dir.join("bin"), &dir.join("x")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
