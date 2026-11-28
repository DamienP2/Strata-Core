//! Sauvegarde et restauration des réglages du Core.
//!
//! Une sauvegarde est une archive `.tar.gz` : `strata-backup.json` (ce qu'elle contient) et les
//! fichiers sous `data/`, tels qu'ils sont dans le dossier des données. Elles sont gardées dans
//! `backups/` : faites à la main, chaque jour (les 14 dernières), et juste avant une restauration.
//!
//! Une restauration n'écrit rien tout de suite : l'archive est déballée dans `.restore/`, puis le
//! Core redémarre et la met en place avant de rien lire ([`apply_pending`]). Aucun réglage en
//! mémoire ne peut donc réécrire l'ancien par-dessus.
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path as UrlPath, State},
    http::{header, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::Read,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use crate::{
    api::{bad, blocking, internal, require, ApiResult},
    auth::CurrentUser,
    logs::now_ms,
    state::Shared,
};

/// Les réglages : toujours sauvegardés et restaurés.
const SETTINGS: &[&str] = &[
    "config.json",
    "users.json",
    "groups.json",
    "hardware.json",
    "streams.json",
    "alerts.json",
    "snmp.json",
    "nmos.json",
    "tls",
    "clock",
    "ndi",
    "storage.json",
    "backup-copy.json",
];
/// Les applis installées, leurs sources et leurs données (au choix : elles peuvent peser lourd).
const APPS: &[&str] = &["apps.json", "sources.json", "apps", "app-data"];
/// L'identité du serveur (sa clé, son rattachement au cockpit) : restaurée seulement si on le
/// demande (sur un autre serveur, ce serait deux serveurs avec la même identité).
const IDENTITY: &[&str] = &["server-key.pk8", "cluster.json"];

const MANIFEST: &str = "strata-backup.json";
/// Les sauvegardes quotidiennes gardées.
const KEEP_DAILY: usize = 14;
/// Une archive envoyée : au plus 4 Go (avec les applis).
const MAX_UPLOAD: u64 = 4 << 30;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Manifest {
    pub format: u32,
    pub core_version: String,
    pub server_name: String,
    pub dns_name: String,
    pub created_at: i64,
    /// manual, daily, before-restore, uploaded.
    pub kind: String,
    pub apps: bool,
}

fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("backups")
}

/// Un nom de sauvegarde sûr (jamais un chemin).
fn checked(name: &str) -> Result<&str, String> {
    let ok = name.ends_with(".tar.gz") && !name.starts_with('.') && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    if ok {
        Ok(name)
    } else {
        Err("unknown backup".into())
    }
}

fn stamp(ms: i64) -> String {
    jiff::Timestamp::from_millisecond(ms)
        .map(|t| t.to_zoned_datetime(jiff::tz::TimeZone::system()).strftime("%Y-%m-%d-%H%M%S").to_string())
        .unwrap_or_else(|_| ms.to_string())
}

/// Fait une sauvegarde dans `backups/` ; rend son nom.
pub fn create(data_dir: &Path, config: &crate::config::ServerConfig, kind: &str, apps: bool) -> Result<String, String> {
    let backups = dir(data_dir);
    make_dir(&backups).map_err(|e| e.to_string())?;
    let now = now_ms();
    let name = format!("strata-{}-{kind}.tar.gz", stamp(now));
    let manifest = Manifest {
        format: 1,
        core_version: strata_core::VERSION.to_string(),
        server_name: config.name.clone(),
        dns_name: config.dns_name.clone(),
        created_at: now,
        kind: kind.to_string(),
        apps,
    };
    let tmp = backups.join(format!(".{name}.part"));
    let r = (|| -> std::io::Result<()> {
        let f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode_private().open(&tmp)?;
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(f, flate2::Compression::default()));
        tar.follow_symlinks(false);
        let m = serde_json::to_vec_pretty(&manifest)?;
        let mut h = tar::Header::new_gnu();
        h.set_size(m.len() as u64);
        h.set_mode(0o600);
        h.set_mtime((now / 1000) as u64);
        h.set_cksum();
        tar.append_data(&mut h, MANIFEST, m.as_slice())?;
        let parts = SETTINGS.iter().chain(IDENTITY).chain(if apps { APPS } else { &[] });
        for p in parts {
            let src = data_dir.join(p);
            match std::fs::symlink_metadata(&src) {
                Ok(md) if md.is_dir() => tar.append_dir_all(format!("data/{p}"), &src)?,
                Ok(md) if md.is_file() => tar.append_path_with_name(&src, format!("data/{p}"))?,
                _ => {}
            }
        }
        tar.into_inner()?.finish()?.sync_all()
    })();
    if let Err(e) = r {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("cannot write the backup: {e}"));
    }
    std::fs::rename(&tmp, backups.join(&name)).map_err(|e| e.to_string())?;
    Ok(name)
}

/// Le dossier des sauvegardes, au seul compte du service (elles contiennent des secrets).
fn make_dir(d: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(d)
}

trait Private {
    fn mode_private(&mut self) -> &mut Self;
}

impl Private for std::fs::OpenOptions {
    fn mode_private(&mut self) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt;
        self.mode(0o600)
    }
}

/// Ce que dit une archive d'elle-même.
fn read_manifest(path: &Path) -> Result<Manifest, String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(f));
    for e in tar.entries().map_err(|_| "not a Strata backup")? {
        let mut e = e.map_err(|_| "not a Strata backup")?;
        if e.path().ok().is_some_and(|p| p == Path::new(MANIFEST)) {
            let mut s = String::new();
            e.by_ref().take(1 << 20).read_to_string(&mut s).map_err(|_| "not a Strata backup")?;
            let m: Manifest = serde_json::from_str(&s).map_err(|_| "not a Strata backup")?;
            if m.format != 1 {
                return Err("this backup comes from a newer Core".into());
            }
            return Ok(m);
        }
    }
    Err("not a Strata backup".into())
}

/// Le premier élément d'un chemin `data/…` s'il fait partie de ce qu'on restaure.
fn wanted<'a>(path: &'a Path, parts: &[&str]) -> Option<&'a Path> {
    let rest = path.strip_prefix("data").ok()?;
    if rest.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    let first = rest.components().next()?.as_os_str().to_str()?;
    parts.contains(&first).then_some(rest)
}

/// Déballe l'archive dans `.restore/` (vérifiée : chemins connus seulement, pas de lien qui
/// sort) ; le Core la met en place à son prochain démarrage.
fn stage(data_dir: &Path, archive: &Path, apps: bool, identity: bool) -> Result<Manifest, String> {
    let m = read_manifest(archive)?;
    let mut parts: Vec<&str> = SETTINGS.to_vec();
    if apps && m.apps {
        parts.extend(APPS);
    }
    if identity {
        parts.extend(IDENTITY);
    }
    let staging = data_dir.join(".restore");
    let _ = std::fs::remove_dir_all(&staging);
    let out = staging.join("data");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let r = (|| -> Result<(), String> {
        let f = std::fs::File::open(archive).map_err(|e| e.to_string())?;
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(f));
        tar.set_preserve_permissions(false);
        tar.set_mask(0o077);
        tar.set_unpack_xattrs(false);
        for e in tar.entries().map_err(|e| e.to_string())? {
            let mut e = e.map_err(|e| e.to_string())?;
            let path = e.path().map_err(|e| e.to_string())?.into_owned();
            let Some(rest) = wanted(&path, &parts) else { continue };
            let kind = e.header().entry_type();
            if kind.is_symlink() {
                // Un lien d'une appli : relatif et sans remonter (il reste dans l'appli).
                let target = e.link_name().ok().flatten().map(|t| t.into_owned()).unwrap_or_default();
                if target.is_absolute() || target.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) {
                    return Err(format!("{}: a link that leaves the backup", path.display()));
                }
            } else if !kind.is_file() && !kind.is_dir() {
                continue;
            }
            let dest = out.join(rest);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            e.unpack(&dest).map_err(|err| format!("{}: {err}", path.display()))?;
        }
        // Ce qui est restauré : les éléments de `parts` présents, et ceux qui ne le sont pas sont
        // effacés (un réglage absent de la sauvegarde revient à son défaut).
        let list: Vec<&str> = parts.clone();
        strata_core::files::write_json(&staging.join("parts.json"), &list, true).map_err(|e| e.to_string())?;
        std::fs::write(staging.join("ready"), b"").map_err(|e| e.to_string())
    })();
    if let Err(e) = r {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    Ok(m)
}

/// Au démarrage, avant de rien lire : met en place une restauration préparée.
pub fn apply_pending(data_dir: &Path) {
    let staging = data_dir.join(".restore");
    if !staging.join("ready").exists() {
        let _ = std::fs::remove_dir_all(&staging);
        return;
    }
    let parts: Vec<String> = std::fs::read_to_string(staging.join("parts.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    for p in &parts {
        if p.contains('/') || p.starts_with('.') {
            continue;
        }
        let dest = data_dir.join(p);
        if dest.is_dir() {
            let _ = std::fs::remove_dir_all(&dest);
        } else {
            let _ = std::fs::remove_file(&dest);
        }
        let src = staging.join("data").join(p);
        if src.exists() {
            if let Err(e) = std::fs::rename(&src, &dest) {
                tracing::error!("restore: cannot put {p} in place: {e}");
            }
        }
    }
    // Les sessions ouvertes étaient celles des anciens comptes.
    let _ = std::fs::remove_file(data_dir.join("sessions.json"));
    let _ = std::fs::remove_dir_all(&staging);
    tracing::warn!("settings restored from a backup ({})", parts.join(", "));
}

/// Une sauvegarde par jour (réglages seuls), les 14 dernières gardées.
pub fn daily(data_dir: &Path, config: &crate::config::ServerConfig) {
    let backups = dir(data_dir);
    let today = stamp(now_ms())[..10].to_string();
    let mut daily: Vec<String> = std::fs::read_dir(&backups)
        .map(|d| d.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| n.ends_with("-daily.tar.gz")).collect())
        .unwrap_or_default();
    if daily.iter().any(|n| n.starts_with(&format!("strata-{today}"))) {
        return;
    }
    match create(data_dir, config, "daily", false) {
        Ok(name) => daily.push(name),
        Err(e) => {
            tracing::warn!("daily backup: {e}");
            return;
        }
    }
    daily.sort();
    let extra = daily.len().saturating_sub(KEEP_DAILY);
    for n in &daily[..extra] {
        let _ = std::fs::remove_file(backups.join(n));
    }
}

/// La sauvegarde quotidienne, revue toutes les heures, puis sa copie vers le serveur de stockage.
pub fn start(s: &Shared) {
    let s = s.clone();
    tokio::spawn(async move {
        loop {
            let s2 = s.clone();
            let _ = tokio::task::spawn_blocking(move || daily(&s2.data_dir, &s2.config.load())).await;
            // La plus récente pas encore copiée (une copie ratée est donc retentée chaque heure).
            if let Err(e) = copy_newest(&s, false).await {
                tracing::debug!("backup copy: {e}");
            }
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
}

// ---------- La copie vers un serveur de stockage ----------

/// Où copier les sauvegardes (Ce serveur › Sauvegardes).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct CopySettings {
    /// Le serveur de stockage (Ce serveur › Stockage) ; vide : pas de copie.
    pub target: String,
    /// Le dossier dans celui du serveur ; vide : le nom du serveur.
    pub folder: String,
    /// Les sauvegardes gardées là-bas (les plus anciennes effacées).
    pub keep: usize,
}

impl Default for CopySettings {
    fn default() -> Self {
        CopySettings { target: String::new(), folder: String::new(), keep: 30 }
    }
}

/// La dernière copie, et celles déjà faites.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct CopyState {
    pub at: i64,
    pub name: String,
    pub ok: bool,
    pub error: String,
    pub sent: Vec<String>,
}

fn copy_settings(data_dir: &Path) -> CopySettings {
    strata_core::files::read_json(&data_dir.join("backup-copy.json")).unwrap_or_default()
}

fn copy_state(data_dir: &Path) -> CopyState {
    strata_core::files::read_json(&data_dir.join("backups").join(".copy.json")).unwrap_or_default()
}

/// Le dossier des copies quand on n'en donne pas : le nom de ce serveur.
fn default_folder(server_name: &str) -> String {
    let n = crate::storage::safe(server_name);
    if n.is_empty() {
        "Strata Core".to_string()
    } else {
        n
    }
}

/// Le dossier là-bas : celui du serveur de stockage, puis le dossier choisi.
fn remote_dir(t: &crate::storage::Target, set: &CopySettings, server_name: &str) -> String {
    let folder = if set.folder.trim().is_empty() {
        default_folder(server_name)
    } else {
        set.folder.split(['/', '\\']).map(crate::storage::safe).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("/")
    };
    let base = crate::storage::Transfer::new(t, Path::new("")).base();
    [base, folder].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("/")
}

static COPYING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Copie la sauvegarde la plus récente (si elle ne l'est pas déjà, ou toujours si `again`) ;
/// rend son nom, ou rien si la copie n'est pas réglée.
pub async fn copy_newest(s: &Shared, again: bool) -> Result<Option<String>, String> {
    let set = copy_settings(&s.data_dir);
    if set.target.is_empty() {
        return Ok(None);
    }
    let d = s.data_dir.clone();
    let newest = tokio::task::spawn_blocking(move || list(&d)).await.map_err(|e| e.to_string())?.into_iter().next();
    let Some(name) = newest.and_then(|v| v["name"].as_str().map(String::from)) else {
        return Ok(None);
    };
    if !again && copy_state(&s.data_dir).sent.contains(&name) {
        return Ok(None);
    }
    copy(s, &name).await.map(Some)
}

/// Copie une sauvegarde vers le serveur de stockage, puis n'y garde que les plus récentes.
pub async fn copy(s: &Shared, name: &str) -> Result<String, String> {
    let _one = COPYING.lock().await;
    let set = copy_settings(&s.data_dir);
    let mut state = copy_state(&s.data_dir);
    let r = async {
        let t = crate::storage::find(&s.data_dir, &set.target).ok_or("the storage server of the backups no longer exists")?;
        let name = checked(name)?.to_string();
        let file = dir(&s.data_dir).join(&name);
        if !file.is_file() {
            return Err("unknown backup".to_string());
        }
        let tr = crate::storage::Transfer::new(&t, &s.data_dir);
        let to = remote_dir(&t, &set, &s.config.load().name);
        tr.put(&file, &to, &name).await?;
        // Les plus anciennes, effacées là-bas (seulement les sauvegardes du Core).
        if set.keep > 0 {
            if let Ok(Some(mut there)) = tr.entries(&to).await {
                there.retain(|(n, is_dir)| !is_dir && n.starts_with("strata-") && checked(n).is_ok());
                there.sort();
                let extra = there.len().saturating_sub(set.keep);
                for (n, _) in &there[..extra] {
                    if let Err(e) = tr.remove(&to, n).await {
                        tracing::warn!("backup copy: cannot remove the old {n}: {e}");
                    }
                }
            }
        }
        Ok(name)
    }
    .await;
    state.at = now_ms();
    state.name = name.to_string();
    state.ok = r.is_ok();
    state.error = r.as_ref().err().cloned().unwrap_or_default();
    if r.is_ok() && !state.sent.iter().any(|n| n == name) {
        state.sent.push(name.to_string());
        let extra = state.sent.len().saturating_sub(200);
        state.sent.drain(..extra);
    }
    let _ = strata_core::files::write_json(&dir(&s.data_dir).join(".copy.json"), &state, true);
    match &r {
        Ok(n) => tracing::info!("backup {n} copied to the storage server"),
        Err(e) => tracing::warn!("backup copy of {name}: {e}"),
    }
    r
}

// ---------- API (administrateurs) ----------

fn list(data_dir: &Path) -> Vec<Value> {
    let mut out: Vec<(i64, Value)> = std::fs::read_dir(dir(data_dir))
        .map(|d| {
            d.flatten()
                .filter_map(|e| {
                    let name = e.file_name().into_string().ok()?;
                    checked(&name).ok()?;
                    let size = e.metadata().ok()?.len();
                    let mut m = read_manifest(&e.path()).ok()?;
                    // Envoyée depuis un navigateur : c'est ce qui compte ici.
                    if name.ends_with("-uploaded.tar.gz") {
                        m.kind = "uploaded".into();
                    }
                    Some((m.created_at, json!({ "name": name, "size": size, "manifest": m })))
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|(t, _)| -t);
    out.into_iter().map(|(_, v)| v).collect()
}

async fn index(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let d = s.data_dir.clone();
    let d2 = s.data_dir.clone();
    let mut state = serde_json::to_value(copy_state(&s.data_dir)).unwrap_or_default();
    state.as_object_mut().map(|o| o.remove("sent"));
    Ok(Json(json!({
        "backups": blocking(move || list(&d)).await?,
        "keepDaily": KEEP_DAILY,
        "copy": blocking(move || copy_settings(&d2)).await?,
        "lastCopy": state,
        "defaultFolder": default_folder(&s.config.load().name),
    })))
}

async fn set_copy(State(s): State<Shared>, CurrentUser(me): CurrentUser, Json(mut set): Json<CopySettings>) -> ApiResult<Json<CopySettings>> {
    require(me.admin)?;
    set.folder = set.folder.trim().to_string();
    if set.folder.split(['/', '\\']).any(|p| p.trim() == "..") {
        return Err(bad("no « .. » in the folders"));
    }
    if !set.target.is_empty() && crate::storage::find(&s.data_dir, &set.target).is_none() {
        return Err(bad("unknown storage server"));
    }
    set.keep = set.keep.min(1000);
    strata_core::files::write_json(&s.data_dir.join("backup-copy.json"), &set, true).map_err(internal)?;
    tracing::info!("backup copy set by {} (storage server: {})", me.username, if set.target.is_empty() { "none" } else { &set.target });
    Ok(Json(set))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct CopyBody {
    /// Une sauvegarde ; sinon la plus récente.
    name: String,
}

/// Copie maintenant (la plus récente, ou celle donnée).
async fn copy_now(State(s): State<Shared>, CurrentUser(me): CurrentUser, Json(b): Json<CopyBody>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    if copy_settings(&s.data_dir).target.is_empty() {
        return Err(bad("choose a storage server first"));
    }
    let name = if b.name.is_empty() {
        copy_newest(&s, true).await.map_err(bad)?.ok_or_else(|| bad("no backup to copy"))?
    } else {
        copy(&s, &b.name).await.map_err(bad)?
    };
    Ok(Json(json!({ "name": name })))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct CreateBody {
    apps: bool,
}

async fn make(State(s): State<Shared>, CurrentUser(me): CurrentUser, Json(b): Json<CreateBody>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let s2 = s.clone();
    let name = blocking(move || create(&s2.data_dir, &s2.config.load(), "manual", b.apps)).await?.map_err(internal)?;
    tracing::info!("backup {name} made by {}", me.username);
    // Copiée vers le serveur de stockage, si c'est réglé, sans faire attendre la page.
    let (s2, n) = (s.clone(), name.clone());
    tokio::spawn(async move {
        if !copy_settings(&s2.data_dir).target.is_empty() {
            let _ = copy(&s2, &n).await;
        }
    });
    Ok(Json(json!({ "name": name })))
}

async fn download(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath(name): UrlPath<String>) -> ApiResult<impl IntoResponse> {
    require(me.admin)?;
    let name = checked(&name).map_err(|e| (StatusCode::NOT_FOUND, e))?.to_string();
    let f = tokio::fs::File::open(dir(&s.data_dir).join(&name)).await.map_err(|_| (StatusCode::NOT_FOUND, "unknown backup".to_string()))?;
    let len = f.metadata().await.map(|m| m.len()).unwrap_or(0);
    let stream = futures_util::stream::unfold(f, |mut f| async move {
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; 256 << 10];
        match f.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok::<_, std::io::Error>(axum::body::Bytes::from(buf)), f))
            }
            Err(e) => Some((Err(e), f)),
        }
    });
    Ok((
        [
            (header::CONTENT_TYPE, "application/gzip".to_string()),
            (header::CONTENT_LENGTH, len.to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")),
        ],
        Body::from_stream(stream),
    ))
}

async fn remove(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath(name): UrlPath<String>) -> ApiResult<StatusCode> {
    require(me.admin)?;
    let name = checked(&name).map_err(|e| (StatusCode::NOT_FOUND, e))?;
    tokio::fs::remove_file(dir(&s.data_dir).join(name)).await.map_err(|_| (StatusCode::NOT_FOUND, "unknown backup".to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Une archive envoyée depuis le navigateur, gardée avec les autres une fois vérifiée.
async fn upload(State(s): State<Shared>, CurrentUser(me): CurrentUser, mut body: Body) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let backups = dir(&s.data_dir);
    make_dir(&backups).map_err(internal)?;
    let tmp = backups.join(format!(".upload-{}.part", strata_core::new_id()));
    let r = async {
        use tokio::io::AsyncWriteExt;
        let mut f = tokio::fs::File::create(&tmp).await.map_err(internal)?;
        let mut size = 0u64;
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|e| bad(format!("the upload was cut: {e}")))?;
            if let Ok(data) = frame.into_data() {
                size += data.len() as u64;
                if size > MAX_UPLOAD {
                    return Err((StatusCode::PAYLOAD_TOO_LARGE, "the archive is too big".to_string()));
                }
                f.write_all(&data).await.map_err(internal)?;
            }
        }
        f.flush().await.map_err(internal)?;
        let t = tmp.clone();
        let m = blocking(move || read_manifest(&t)).await?.map_err(bad)?;
        let name = format!("strata-{}-uploaded.tar.gz", stamp(now_ms()));
        tokio::fs::rename(&tmp, backups.join(&name)).await.map_err(internal)?;
        Ok(json!({ "name": name, "manifest": m }))
    }
    .await;
    if r.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    Ok(Json(r?))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct RestoreBody {
    /// Les applis aussi (si la sauvegarde les a).
    apps: bool,
    /// L'identité du serveur aussi (le même serveur, réinstallé).
    identity: bool,
}

/// Prépare la restauration (après une sauvegarde de l'état actuel), puis redémarre le Core.
async fn restore(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath(name): UrlPath<String>, Json(b): Json<RestoreBody>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let name = checked(&name).map_err(|e| (StatusCode::NOT_FOUND, e))?.to_string();
    let archive = dir(&s.data_dir).join(&name);
    if !archive.is_file() {
        return Err((StatusCode::NOT_FOUND, "unknown backup".into()));
    }
    let s2 = s.clone();
    let m = blocking(move || -> Result<Manifest, String> {
        // L'état actuel d'abord : on peut revenir en arrière.
        create(&s2.data_dir, &s2.config.load(), "before-restore", b.apps)?;
        stage(&s2.data_dir, &archive, b.apps, b.identity)
    })
    .await?
    .map_err(bad)?;
    tracing::warn!("restore of {name} asked by {} (apps: {}, identity: {}): restarting", me.username, b.apps && m.apps, b.identity);
    let s3 = s.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        s3.ask_restart();
    });
    Ok(Json(json!({ "manifest": m })))
}

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/backups", get(index).post(make))
        .route("/api/backups/copy", axum::routing::put(set_copy).post(copy_now))
        .route("/api/backups/upload", post(upload).layer(DefaultBodyLimit::disable()))
        .route("/api/backups/{name}", get(download).delete(remove))
        .route("/api/backups/{name}/restore", post(restore))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_go_in_a_folder_of_the_server() {
        let t = crate::storage::Target {
            name: "NAS".into(),
            kind: "smb".into(),
            host: "nas".into(),
            share: "S".into(),
            path: "/Strata/".into(),
            ..Default::default()
        };
        let set = CopySettings { target: "x".into(), folder: String::new(), keep: 3 };
        assert_eq!(remote_dir(&t, &set, "Régie A"), "Strata/Régie A");
        let set = CopySettings { folder: "a/../b:c/".into(), ..set };
        assert_eq!(remote_dir(&t, &set, "Régie A"), "Strata/a/b-c");
        let d = crate::storage::Target { kind: "dir".into(), path: "/mnt/nas".into(), ..t };
        assert_eq!(remote_dir(&d, &CopySettings::default(), ""), "Strata Core");
    }

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("strata-backup-test-{}", strata_core::new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn backs_up_and_restores_at_the_next_start() {
        let a = tmp();
        std::fs::write(a.join("config.json"), r#"{"name":"Régie A"}"#).unwrap();
        std::fs::write(a.join("users.json"), "[1]").unwrap();
        std::fs::create_dir_all(a.join("tls")).unwrap();
        std::fs::write(a.join("tls/strata.key"), "key").unwrap();
        std::fs::write(a.join("server-key.pk8"), "id-a").unwrap();
        std::fs::write(a.join("sessions.json"), "{}").unwrap();
        std::fs::create_dir_all(a.join("apps/cut/bin")).unwrap();
        std::fs::write(a.join("apps/cut/bin/cut"), "bin").unwrap();
        let cfg = crate::config::ServerConfig { name: "Régie A".into(), ..Default::default() };
        let name = create(&a, &cfg, "manual", false).unwrap();
        let m = read_manifest(&dir(&a).join(&name)).unwrap();
        assert_eq!((m.server_name.as_str(), m.apps), ("Régie A", false));

        // Un autre serveur : ses réglages sont remplacés, pas son identité ni ses applis.
        let b = tmp();
        std::fs::write(b.join("config.json"), r#"{"name":"Régie B"}"#).unwrap();
        std::fs::write(b.join("snmp.json"), "{}").unwrap();
        std::fs::write(b.join("server-key.pk8"), "id-b").unwrap();
        std::fs::write(b.join("sessions.json"), "{}").unwrap();
        std::fs::create_dir_all(b.join("apps/other")).unwrap();
        stage(&b, &dir(&a).join(&name), true, false).unwrap();
        // Rien de changé avant le redémarrage.
        assert!(std::fs::read_to_string(b.join("config.json")).unwrap().contains("Régie B"));
        apply_pending(&b);
        assert!(std::fs::read_to_string(b.join("config.json")).unwrap().contains("Régie A"));
        assert_eq!(std::fs::read_to_string(b.join("tls/strata.key")).unwrap(), "key");
        assert_eq!(std::fs::read_to_string(b.join("users.json")).unwrap(), "[1]");
        assert!(!b.join("snmp.json").exists(), "a setting missing from the backup is reset");
        assert_eq!(std::fs::read_to_string(b.join("server-key.pk8")).unwrap(), "id-b");
        assert!(b.join("apps/other").exists() && !b.join("apps/cut").exists());
        assert!(!b.join("sessions.json").exists() && !b.join(".restore").exists());

        // Avec les applis et l'identité.
        let name = create(&a, &cfg, "manual", true).unwrap();
        stage(&b, &dir(&a).join(&name), true, true).unwrap();
        apply_pending(&b);
        assert_eq!(std::fs::read_to_string(b.join("apps/cut/bin/cut")).unwrap(), "bin");
        assert_eq!(std::fs::read_to_string(b.join("server-key.pk8")).unwrap(), "id-a");
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn refuses_what_is_not_a_backup_or_leaves_it() {
        let a = tmp();
        assert!(checked("../config.json").is_err() && checked(".x.tar.gz").is_err() && checked("strata-1-manual.tar.gz").is_ok());
        assert_eq!(wanted(Path::new("data/config.json"), SETTINGS), Some(Path::new("config.json")));
        assert_eq!(wanted(Path::new("data/../etc/passwd"), SETTINGS), None);
        assert_eq!(wanted(Path::new("data/apps/x"), SETTINGS), None);
        // Une archive sans manifeste.
        let p = a.join("x.tar.gz");
        let f = std::fs::File::create(&p).unwrap();
        let mut t = tar::Builder::new(flate2::write::GzEncoder::new(f, flate2::Compression::fast()));
        let mut h = tar::Header::new_gnu();
        h.set_size(2);
        h.set_cksum();
        t.append_data(&mut h, "data/config.json", &b"{}"[..]).unwrap();
        t.into_inner().unwrap().finish().unwrap();
        assert!(stage(&a, &p, false, false).is_err());
        let _ = std::fs::remove_dir_all(&a);
    }
}
