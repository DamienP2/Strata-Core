//! Les fichiers de données : JSON lisible, écrit de façon atomique (`.tmp` puis renommage), les
//! fichiers sensibles en 0600. Repris de LiveCut (`paths.rs`).
use std::path::Path;

/// Écrit `value` en JSON lisible, de façon atomique ; `private` : lisible par le seul propriétaire.
pub fn write_json<T: serde::Serialize + ?Sized>(path: &Path, value: &T, private: bool) -> std::io::Result<()> {
    let data = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    write_atomic(path, &data, private)
}

/// Écrit un fichier de façon atomique : jamais à moitié écrit, même si le serveur s'arrête, ni
/// mêlé quand deux écrivains passent en même temps (chacun son fichier temporaire ; le dernier
/// renommé gagne).
pub fn write_atomic(path: &Path, data: &[u8], private: bool) -> std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
    if let Some(dir) = dir {
        std::fs::create_dir_all(dir)?;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{}.{}.tmp", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
    let written = (|| {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(if private { 0o600 } else { 0o644 });
        }
        #[cfg(not(unix))]
        let _ = private;
        let mut f = opts.open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
        return written;
    }
    // Le renommage lui-même tient après une coupure.
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir.unwrap_or(Path::new("."))) {
        let _ = d.sync_all();
    }
    Ok(())
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
