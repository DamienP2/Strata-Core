//! Les mises à jour (Réglages › Ce serveur › Mises à jour) : chercher les nouvelles versions du Core
//! et des applis installées depuis GitHub (à la demande, et chaque jour si on le veut), et mettre à
//! jour le Core lui-même.
//!
//! Le Core se met à jour ainsi : la release la plus récente de son dépôt (hors préversions), son
//! archive téléchargée et son empreinte vérifiée ; le nouveau programme est écrit à côté de
//! l'actuel, lancé une fois (`--version`, qui doit dire la version attendue), puis mis à sa place,
//! l'ancien gardé (`strata-core.previous`) ; enfin le Core redémarre. Même chose depuis une archive
//! envoyée (serveur sans internet). Avant, l'interface prévient si une appli installée n'accepterait
//! pas la nouvelle version. Les applis, elles, se mettent à jour par `install.rs`, et seulement vers
//! une version qui accepte le Core en place.
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    routing::{get, post, put},
    Json, Router,
};
use http_body_util::BodyExt;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant, SystemTime},
};
use strata_core::manifest::accepts;

use crate::{
    api::{bad, blocking, require, ApiResult},
    auth::CurrentUser,
    install::{self, Job, ReleaseFile, Step, TempFile},
    state::Shared,
};

/// Une appli qui n'accepterait pas la nouvelle version du Core.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Blocked {
    pub id: String,
    pub name: String,
    pub version: String,
    /// Les versions du Core qu'elle accepte.
    pub needs: String,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct CoreCheck {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest: Option<String>,
    pub newer: bool,
    pub blocked: Vec<Blocked>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppCheck {
    /// La version la plus récente qui accepte ce Core.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest: Option<String>,
    pub newer: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Le résultat de la dernière recherche.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Checked {
    /// Quand (secondes Unix).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub core: Option<CoreCheck>,
    pub apps: BTreeMap<String, AppCheck>,
}

impl Checked {
    /// Combien de mises à jour attendent.
    pub fn count(&self) -> usize {
        usize::from(self.core.as_ref().is_some_and(|c| c.newer)) + self.apps.values().filter(|a| a.newer).count()
    }
}

pub struct Updates {
    /// Le programme du Core (retenu au démarrage : remplacé, son chemin ne se relit plus).
    exe: Mutex<PathBuf>,
    checked: Mutex<Checked>,
    job: Mutex<Option<Job>>,
    checking: tokio::sync::Mutex<()>,
    /// La dernière recherche automatique.
    last_auto: Mutex<Option<Instant>>,
}

impl Default for Updates {
    fn default() -> Self {
        Updates {
            exe: Mutex::new(std::env::current_exe().unwrap_or_else(|_| PathBuf::from("strata-core"))),
            checked: Mutex::default(),
            job: Mutex::default(),
            checking: tokio::sync::Mutex::new(()),
            last_auto: Mutex::default(),
        }
    }
}

impl Updates {
    pub fn exe(&self) -> PathBuf {
        self.exe.lock().unwrap().clone()
    }

    /// Un autre programme à remplacer (les tests).
    pub fn set_exe(&self, path: PathBuf) {
        *self.exe.lock().unwrap() = path;
    }

    pub fn checked(&self) -> Checked {
        self.checked.lock().unwrap().clone()
    }

    fn job(&self) -> Option<Job> {
        self.job.lock().unwrap().clone()
    }

    fn set_job(&self, f: impl FnOnce(&mut Job)) {
        if let Some(j) = self.job.lock().unwrap().as_mut() {
            f(j);
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn core_version() -> Version {
    Version::parse(strata_core::VERSION).expect("the Core's version")
}

/// La release du Core la plus récente (hors brouillons et préversions) : celle dont la fiche
/// s'appelle `strata-core`.
fn pick_core(agent: &ureq::Agent, api: &str, repo: &str, key: &str) -> Result<(install::GhRelease, ReleaseFile), String> {
    let mut res = install::gh_get(agent, &format!("{api}/repos/{repo}/releases?per_page=30"), repo, key, "application/vnd.github+json")?;
    let bytes = res.body_mut().with_config().limit(16 << 20).read_to_vec().map_err(|e| format!("GitHub's answer is unreadable: {e}"))?;
    let list: Vec<install::GhRelease> = serde_json::from_slice(&bytes).map_err(|e| format!("GitHub's answer is unreadable: {e}"))?;
    for r in list.into_iter().filter(|r| !r.draft && !r.prerelease) {
        let Some(a) = r.assets.iter().find(|a| a.name == "release.json") else { continue };
        let mut res = install::gh_get(agent, &install::asset_url(api, repo, a.id), repo, key, "application/octet-stream")?;
        let bytes = res.body_mut().with_config().limit(1 << 20).read_to_vec().map_err(|e| format!("cannot read release.json of {}: {e}", r.tag_name))?;
        let file: ReleaseFile = serde_json::from_slice(&bytes).map_err(|e| format!("release.json of {} is unreadable: {e}", r.tag_name))?;
        if file.name == "strata-core" {
            return Ok((r, file));
        }
    }
    Err(format!("{repo} has no release of the Core"))
}

/// Cherche les nouvelles versions du Core et des applis installées depuis GitHub.
pub fn check(s: &Shared) -> Checked {
    let agent = install::agent();
    let api = s.installer.api();
    let repo = s.config.load().updates.core_repo.clone();
    let apps = s.apps.list();
    let core = match pick_core(&agent, &api, &repo, &s.installer.core_key()) {
        Ok((_, file)) => {
            let newer = file.version > core_version();
            let blocked = if newer {
                apps.iter()
                    .filter(|a| !accepts(&a.manifest.requires.core, &file.version))
                    .map(|a| Blocked {
                        id: a.manifest.id.clone(),
                        name: a.manifest.name.clone(),
                        version: a.manifest.version.to_string(),
                        needs: a.manifest.requires.core.to_string(),
                    })
                    .collect()
            } else {
                Vec::new()
            };
            CoreCheck { latest: Some(file.version.to_string()), newer, blocked, error: None }
        }
        Err(e) => CoreCheck { error: Some(e), ..CoreCheck::default() },
    };
    let mut out = BTreeMap::new();
    for (id, src) in s.installer.sources() {
        let Some(app) = apps.iter().find(|a| a.manifest.id == id) else { continue };
        let c = match install::pick(&agent, &api, &src.repo, &src.key) {
            Ok((_, file)) => AppCheck { latest: Some(file.version.to_string()), newer: file.name == id && file.version > app.manifest.version, error: None },
            Err(e) => AppCheck { error: Some(e), ..AppCheck::default() },
        };
        out.insert(id, c);
    }
    let checked = Checked { at: Some(unix_now()), core: Some(core), apps: out };
    *s.updates.checked.lock().unwrap() = checked.clone();
    tracing::info!("updates checked: {} available", checked.count());
    checked
}

/// La recherche quotidienne (si elle est voulue) : 10 minutes après le démarrage, puis toutes les
/// 24 heures. Appelée régulièrement par le service.
pub async fn tick(s: &Shared) {
    if !s.config.load().updates.daily || s.started_at.elapsed() < Duration::from_secs(600) {
        return;
    }
    {
        let mut last = s.updates.last_auto.lock().unwrap();
        if last.is_some_and(|t| t.elapsed() < Duration::from_secs(24 * 3600)) {
            return;
        }
        *last = Some(Instant::now());
    }
    let Ok(_one) = s.updates.checking.try_lock() else { return };
    let s2 = s.clone();
    let _ = tokio::task::spawn_blocking(move || check(&s2)).await;
}

/// Le nom du programme et sa version, d'après `<programme> --version` (« strata-core 0.3.0 »).
fn version_of(program: &Path) -> Result<Version, String> {
    let mut child = Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("the new Core does not start: {e}"))?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < Duration::from_secs(10) => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                return Err("the new Core does not answer".into());
            }
        }
    }
    let mut out = String::new();
    child.stdout.take().map(|mut o| o.read_to_string(&mut out));
    let v = out.trim().strip_prefix("strata-core ").ok_or_else(|| "the new program is not the Core".to_string())?;
    Version::parse(v.trim()).map_err(|_| "the new program is not the Core".to_string())
}

/// Met le Core d'une archive (`strata-core-<v>-linux-x86_64.tar.gz`) à la place de `exe` : écrit à
/// côté, essayé, puis mis en place ; l'ancien reste en `<exe>.previous`. Rend la nouvelle version.
pub fn replace_binary(exe: &Path, archive: &Path, expected: Option<&Version>) -> Result<Version, String> {
    let dir = exe.parent().ok_or("the Core's folder is unknown")?;
    let name = exe.file_name().ok_or("the Core's name is unknown")?.to_string_lossy().to_string();
    let new = TempFile(dir.join(format!(".{name}.new")));
    let mut f = std::fs::File::open(archive).map_err(|e| format!("cannot open the archive: {e}"))?;
    let mut magic = [0u8; 2];
    if f.read_exact(&mut magic).is_err() || magic != [0x1f, 0x8b] {
        return Err("this is not a .tar.gz archive".into());
    }
    std::io::Seek::rewind(&mut f).map_err(|e| e.to_string())?;
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(std::io::BufReader::new(f)));
    let mut found = false;
    for e in ar.entries().map_err(|_| "this is not a .tar.gz archive".to_string())? {
        let mut e = e.map_err(|_| "the archive is damaged".to_string())?;
        let is_core = e.header().entry_type().is_file() && e.path().ok().and_then(|p| p.file_name().map(|n| n == "strata-core")).unwrap_or(false);
        if !is_core {
            continue;
        }
        if e.size() > install::MAX_ARCHIVE {
            return Err("the archive is too big".into());
        }
        let mut out = std::fs::File::create(&new.0).map_err(|e| format!("cannot write in {}: {e}", dir.display()))?;
        std::io::copy(&mut e, &mut out).map_err(|_| "the archive is damaged".to_string())?;
        out.flush().map_err(|e| e.to_string())?;
        out.sync_all().map_err(|e| e.to_string())?;
        found = true;
        break;
    }
    if !found {
        return Err("the archive holds no strata-core program".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new.0, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    let version = version_of(&new.0)?;
    if let Some(want) = expected {
        if *want != version {
            return Err(format!("the archive holds the Core {version}, not {want}"));
        }
    }
    if version <= core_version() {
        return Err(format!("this archive holds the Core {version}; this one is already {}", strata_core::VERSION));
    }
    let previous = dir.join(format!("{name}.previous"));
    std::fs::copy(exe, &previous).map_err(|e| format!("cannot keep the current Core: {e}"))?;
    std::fs::rename(&new.0, exe).map_err(|e| format!("cannot replace {}: {e}", exe.display()))?;
    // Les bibliothèques livrées avec lui (MXL, MTL), à côté : `<bin>/../lib`.
    match put_libs(archive, &dir.join("../lib")) {
        Ok(n) if n > 0 => tracing::info!("{n} library files put in {}", dir.join("../lib").display()),
        Ok(_) => {}
        Err(e) => tracing::warn!("libraries of the new Core not put in place: {e}"),
    }
    tracing::info!("Core {version} put in place of {} (previous kept)", strata_core::VERSION);
    Ok(version)
}

/// Sort `lib/` de l'archive du Core (MXL ; MTL et DPDK sous `mtl/`) dans `dir`.
fn put_libs(archive: &Path, dir: &Path) -> Result<usize, String> {
    let f = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(std::io::BufReader::new(f)));
    let mut n = 0;
    for e in ar.entries().map_err(|_| "not a .tar.gz archive".to_string())? {
        let mut e = e.map_err(|_| "the archive is damaged".to_string())?;
        let p = e.path().map_err(|e| e.to_string())?.to_path_buf();
        // `strata-core-x.y.z-linux-x86_64/lib/<chemin>` : MXL, et MTL avec DPDK sous `mtl/`.
        let mut parts = p.components();
        parts.next();
        if parts.next().map(|c| c.as_os_str()) != Some(std::ffi::OsStr::new("lib")) {
            continue;
        }
        let rest: std::path::PathBuf = parts.collect();
        if rest.as_os_str().is_empty() || rest.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
            continue;
        }
        let to = dir.join(&rest);
        if e.header().entry_type().is_dir() {
            std::fs::create_dir_all(&to).map_err(|e| format!("cannot create {}: {e}", to.display()))?;
            continue;
        }
        let parent = to.parent().unwrap_or(dir);
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        // Posé à côté puis renommé : un Core qui tourne garde l'ancienne en mémoire.
        let tmp = parent.join(format!(".{}.new", to.file_name().unwrap_or_default().to_string_lossy()));
        let _ = std::fs::remove_file(&tmp);
        e.unpack(&tmp).map_err(|e| format!("cannot write {}: {e}", to.display()))?;
        std::fs::rename(&tmp, &to).map_err(|e| format!("cannot write {}: {e}", to.display()))?;
        n += 1;
    }
    Ok(n)
}

/// Lance la mise à jour du Core depuis GitHub, en tâche de fond ; le Core redémarre à la fin.
pub fn start_core(s: &Shared) -> Result<Job, String> {
    let repo = s.config.load().updates.core_repo.clone();
    let job = Job::new(&repo);
    {
        let mut slot = s.updates.job.lock().unwrap();
        if slot.as_ref().is_some_and(|j| j.running()) {
            return Err("the Core is already being updated".into());
        }
        *slot = Some(job.clone());
    }
    let s = s.clone();
    tokio::spawn(async move {
        let s2 = s.clone();
        let r = tokio::task::spawn_blocking(move || fetch_core(&s2, &repo)).await.unwrap_or_else(|e| Err(e.to_string()));
        match r {
            Ok(v) => {
                s.updates.set_job(|j| {
                    j.version = Some(v.to_string());
                    j.finish(Step::Done);
                });
                restart_soon(&s);
            }
            Err(e) => {
                tracing::warn!("Core update failed: {e}");
                s.updates.set_job(|j| {
                    j.error = Some(e);
                    j.finish(Step::Failed);
                });
            }
        }
    });
    Ok(job)
}

fn fetch_core(s: &Shared, repo: &str) -> Result<Version, String> {
    let agent = install::agent();
    let api = s.installer.api();
    let key = s.installer.core_key();
    let (release, file) = pick_core(&agent, &api, repo, &key)?;
    if file.version <= core_version() {
        return Err(format!("this Core is already the latest ({})", strata_core::VERSION));
    }
    let wanted = file
        .assets
        .iter()
        .find(|a| a.kind == "core-linux-x86_64")
        .or_else(|| file.assets.iter().find(|a| a.name.starts_with("strata-core-") && a.name.ends_with("-linux-x86_64.tar.gz")))
        .ok_or_else(|| format!("release {} has no Core archive", release.tag_name))?;
    let asset = release.assets.iter().find(|a| a.name == wanted.name).ok_or_else(|| format!("{} is missing from release {}", wanted.name, release.tag_name))?;
    s.updates.set_job(|j| {
        j.step = Step::Downloading;
        j.version = Some(file.version.to_string());
        j.total = asset.size;
    });
    let tmp = TempFile(s.data_dir.join(format!(".core-{}.tar.gz", strata_core::new_id())));
    let sum = install::download(&agent, &install::asset_url(&api, repo, asset.id), repo, &key, &tmp.0, |done, total| {
        s.updates.set_job(|j| {
            j.done = done;
            j.total = total;
        })
    })?;
    if !sum.eq_ignore_ascii_case(wanted.sha256.trim()) {
        return Err("the download does not match its fingerprint".into());
    }
    s.updates.set_job(|j| j.step = Step::Installing);
    replace_binary(&s.updates.exe(), &tmp.0, Some(&file.version))
}

/// Le temps que l'interface voie la fin, puis le redémarrage (le nouveau programme).
fn restart_soon(s: &Shared) {
    let s = s.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        s.ask_restart();
    });
}

// ---------- API (administrateurs) ----------

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/updates", get(read))
        .route("/api/updates/check", post(check_now))
        .route("/api/updates/settings", put(settings))
        .route("/api/updates/core", post(update_core))
        .route("/api/updates/core/upload", post(upload_core).layer(DefaultBodyLimit::disable()))
        .route("/api/updates/core/key", put(core_key))
}

fn state(s: &Shared) -> Value {
    let cfg = s.config.load();
    let checked = s.updates.checked();
    json!({
        "core": {
            "version": strata_core::VERSION,
            "exe": s.updates.exe().display().to_string(),
            "repo": cfg.updates.core_repo,
            "key": !s.installer.core_key().is_empty(),
        },
        "daily": cfg.updates.daily,
        "count": checked.count(),
        "checked": checked,
        "job": s.updates.job(),
    })
}

async fn read(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    Ok(Json(state(&s)))
}

async fn check_now(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let _one = s.updates.checking.lock().await;
    let s2 = s.clone();
    blocking(move || check(&s2)).await?;
    Ok(Json(state(&s)))
}

#[derive(Deserialize)]
struct SettingsBody {
    daily: bool,
}

async fn settings(State(s): State<Shared>, CurrentUser(me): CurrentUser, Json(b): Json<SettingsBody>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let mut cfg = (**s.config.load()).clone();
    cfg.updates.daily = b.daily;
    let dir = s.data_dir.clone();
    let saved = cfg.clone();
    blocking(move || saved.save(&dir)).await?.map_err(|e| bad(format!("cannot save the settings: {e}")))?;
    s.config.store(std::sync::Arc::new(cfg));
    Ok(Json(state(&s)))
}

async fn update_core(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<(StatusCode, Json<Value>)> {
    require(me.admin)?;
    start_core(&s).map_err(bad)?;
    tracing::info!("Core update asked by {}", me.username);
    Ok((StatusCode::ACCEPTED, Json(state(&s))))
}

/// Une archive du Core envoyée par le navigateur (serveur sans internet) ; le Core redémarre.
async fn upload_core(State(s): State<Shared>, CurrentUser(me): CurrentUser, mut body: Body) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let tmp = TempFile(s.data_dir.join(format!(".core-upload-{}.tar.gz", strata_core::new_id())));
    {
        use tokio::io::AsyncWriteExt;
        let mut f = tokio::fs::File::create(&tmp.0).await.map_err(crate::api::internal)?;
        let mut size = 0u64;
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|e| bad(format!("the upload was cut: {e}")))?;
            if let Ok(data) = frame.into_data() {
                size += data.len() as u64;
                if size > install::MAX_ARCHIVE {
                    return Err((StatusCode::PAYLOAD_TOO_LARGE, "the archive is too big".into()));
                }
                f.write_all(&data).await.map_err(crate::api::internal)?;
            }
        }
        f.flush().await.map_err(crate::api::internal)?;
    }
    let exe = s.updates.exe();
    let v = blocking(move || replace_binary(&exe, &tmp.0, None)).await?.map_err(bad)?;
    tracing::info!("Core {v} installed from an archive by {}", me.username);
    restart_soon(&s);
    Ok(Json(json!({ "version": v.to_string() })))
}

#[derive(Deserialize)]
struct KeyBody {
    key: String,
}

async fn core_key(State(s): State<Shared>, CurrentUser(me): CurrentUser, Json(b): Json<KeyBody>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    s.installer.set_core_key(&b.key).map_err(crate::api::internal)?;
    tracing::info!("key of the Core's repository changed by {}", me.username);
    Ok(Json(state(&s)))
}
