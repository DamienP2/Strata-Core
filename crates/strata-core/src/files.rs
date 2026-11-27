//! Les fichiers de données : JSON lisible, écrit de façon atomique (`.tmp` puis renommage), les
//! fichiers sensibles en 0600. Repris de LiveCut (`paths.rs`).
use std::path::{Path, PathBuf};

/// Écrit `value` en JSON lisible, de façon atomique ; `private` : lisible par le seul propriétaire.
pub fn write_json<T: serde::Serialize + ?Sized>(path: &Path, value: &T, private: bool) -> std::io::Result<()> {
    let data = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    write_atomic(path, &data, private)
}

/// Écrit un fichier de façon atomique : jamais à moitié écrit, même si le serveur s'arrête.
pub fn write_atomic(path: &Path, data: &[u8], private: bool) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, data)?;
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = private;
    std::fs::rename(&tmp, path)
}

/// Lit un fichier JSON ; absent : `None` ; illisible : `None`, et l'erreur au journal.
pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let data = std::fs::read(path).ok()?;
    match serde_json::from_slice(&data) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::error!("{}: {e}", path.display());
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_json() {
        let dir = std::env::temp_dir().join(format!("strata-files-{}", crate::new_id()));
        let p = dir.join("sub/data.json");
        write_json(&p, &serde_json::json!({"a": 1}), true).unwrap();
        let v: serde_json::Value = read_json(&p).unwrap();
        assert_eq!(v["a"], 1);
        assert!(!p.with_extension("json.tmp").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert!(read_json::<serde_json::Value>(&dir.join("absent.json")).is_none());
        std::fs::write(dir.join("bad.json"), b"{").unwrap();
        assert!(read_json::<serde_json::Value>(&dir.join("bad.json")).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
