//! Installer, mettre à jour et désinstaller une appli (la Bibliothèque).
//!
//! - **Depuis GitHub** : le dépôt de l'appli (`DamienP2/Strata-Cut`), et sa clé s'il est privé (un
//!   jeton en lecture seule, gardé dans `sources.json`, jamais renvoyé au navigateur). Le Core prend
//!   la release la plus récente, hors préversions, qui accepte sa version (sa fiche `release.json`),
//!   télécharge l'archive de l'appli et vérifie son empreinte SHA-256.
//! - **Depuis une archive envoyée** (serveur sans internet) : la même archive.
//!
//! L'archive (`<id>-<version>-linux-x86_64.tar.gz`) contient la fiche `strata-app.toml` et les fichiers
//! de l'appli, à sa racine ou dans un seul dossier. Elle est dépaquetée à côté, vérifiée, puis mise en
//! place d'un coup à la place de l'ancienne version ; une appli active est relancée. Ses données
//! (`app-data/<id>`) ne sont jamais touchées, sauf à la désinstallation si on le demande.
//! Format des releases : docs/APPLIS.md.
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path as UrlPath, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post, put},
    Json, Router,
};
use http_body_util::BodyExt;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, RwLock},
    time::{Duration, Instant},
};
use strata_core::manifest::{accepts, AppManifest};

use crate::{
    api::{bad, blocking, require, ApiResult},
    apps::{app_json, launch_of, Installed},
    auth::{host, CurrentUser},
    state::Shared,
};

/// L'archive d'une appli : 2 Gio au plus ; dépaquetée : 8 Gio au plus.
pub(crate) const MAX_ARCHIVE: u64 = 2 << 30;
const MAX_UNPACKED: u64 = 8 << 30;
const DEFAULT_API: &str = "https://api.github.com";

/// D'où vient une appli : son dépôt GitHub, et sa clé (dépôt privé).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Source {
    pub repo: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
}

#[derive(Serialize, Deserialize, Default)]
struct SourcesFile {
    #[serde(default)]
    apps: BTreeMap<String, Source>,
    /// La clé du dépôt du Core (dépôt privé), pour ses mises à jour.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    core_key: String,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Step {
    /// Cherche la release à installer.
    Looking,
    Downloading,
    /// Vérifie, dépaquette, met en place.
    Installing,
    Done,
    Failed,
}

/// Une installation depuis GitHub, en cours ou finie depuis peu.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub repo: String,
    pub step: Step,
    /// Octets téléchargés, sur `total`.
    pub done: u64,
    pub total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// L'appli, une fois connue.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip)]
    ended: Option<Instant>,
}

impl Job {
    /// Une installation qui commence.
    pub fn new(repo: &str) -> Self {
        Job { repo: repo.to_string(), step: Step::Looking, done: 0, total: 0, version: None, app: None, error: None, ended: None }
    }

    pub fn finish(&mut self, step: Step) {
        self.step = step;
        self.ended = Some(Instant::now());
    }

    pub fn running(&self) -> bool {
        self.ended.is_none()
    }
}

pub struct Installer {
    jobs: Mutex<HashMap<String, Job>>,
    api: RwLock<String>,
    /// Une seule mise en place à la fois.
    swap: Mutex<()>,
    sources: PathBuf,
}

impl Installer {
    pub fn new(data_dir: &Path) -> Self {
        let api = std::env::var("STRATA_GITHUB_API").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| DEFAULT_API.into());
        Installer { jobs: Mutex::default(), api: RwLock::new(api), swap: Mutex::default(), sources: data_dir.join("sources.json") }
    }

    /// L'adresse de l'API GitHub (une autre pour les tests ou un GitHub Enterprise).
    pub fn set_api(&self, url: &str) {
        *self.api.write().unwrap() = url.trim_end_matches('/').to_string();
    }

    pub(crate) fn api(&self) -> String {
        self.api.read().unwrap().clone()
    }

    /// Les installations en cours, et celles finies depuis peu (pour que l'interface voie la fin).
    pub fn jobs(&self) -> Vec<Job> {
        let mut jobs = self.jobs.lock().unwrap();
        jobs.retain(|_, j| match (j.step, j.ended) {
            (Step::Done, Some(t)) => t.elapsed() < Duration::from_secs(20),
            (Step::Failed, Some(t)) => t.elapsed() < Duration::from_secs(600),
            _ => true,
        });
        let mut list: Vec<Job> = jobs.values().cloned().collect();
        list.sort_by(|a, b| a.repo.cmp(&b.repo));
        list
    }

    fn job(&self, repo: &str, f: impl FnOnce(&mut Job)) {
        if let Some(j) = self.jobs.lock().unwrap().get_mut(&repo.to_lowercase()) {
            f(j);
            if matches!(j.step, Step::Done | Step::Failed) {
                j.ended = Some(Instant::now());
            }
        }
    }

    /// Oublie une installation finie (l'interface a montré son résultat).
    pub fn dismiss(&self, repo: &str) {
        let mut jobs = self.jobs.lock().unwrap();
        if jobs.get(&repo.to_lowercase()).is_some_and(|j| j.ended.is_some()) {
            jobs.remove(&repo.to_lowercase());
        }
    }

    fn read_sources(&self) -> SourcesFile {
        strata_core::files::read_json(&self.sources).unwrap_or_default()
    }

    /// La clé du dépôt du Core.
    pub fn core_key(&self) -> String {
        self.read_sources().core_key
    }

    pub fn set_core_key(&self, key: &str) -> Result<(), String> {
        let mut f = self.read_sources();
        f.core_key = key.trim().to_string();
        strata_core::files::write_json(&self.sources, &f, true).map_err(|e| format!("cannot save the sources: {e}"))
    }

    /// Les applis installées depuis GitHub, et leur dépôt.
    pub fn sources(&self) -> BTreeMap<String, Source> {
        self.read_sources().apps
    }

    pub fn source(&self, id: &str) -> Option<Source> {
        self.read_sources().apps.get(id).cloned()
    }

    fn set_source(&self, id: &str, src: Option<Source>) -> Result<(), String> {
        let mut f = self.read_sources();
        match src {
            Some(s) => f.apps.insert(id.to_string(), s),
            None => f.apps.remove(id),
        };
        strata_core::files::write_json(&self.sources, &f, true).map_err(|e| format!("cannot save the sources: {e}"))
    }

    /// La clé gardée pour ce dépôt (celle d'une appli déjà installée depuis lui).
    fn key_of(&self, repo: &str) -> String {
        self.read_sources().apps.values().find(|s| s.repo.eq_ignore_ascii_case(repo)).map(|s| s.key.clone()).unwrap_or_default()
    }
}

/// `owner/nom`, ou l'adresse GitHub du dépôt.
pub fn parse_repo(text: &str) -> Result<String, String> {
    let t = text.trim().trim_end_matches('/');
    let t = t.strip_prefix("https://").or_else(|| t.strip_prefix("http://")).unwrap_or(t);
    let t = t.strip_prefix("www.").unwrap_or(t);
    let t = t.strip_prefix("github.com/").unwrap_or(t);
    let t = t.strip_suffix(".git").unwrap_or(t);
    let ok_part = |p: &str| !p.is_empty() && p.len() <= 100 && p != "." && p != ".." && p.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    match t.split_once('/') {
        Some((o, r)) if ok_part(o) && ok_part(r) => Ok(format!("{o}/{r}")),
        _ => Err(format!("not a GitHub repository: {text}")),
    }
}

// ---------- GitHub ----------

#[derive(Deserialize)]
pub(crate) struct GhRelease {
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<GhAsset>,
}

#[derive(Deserialize, Clone)]
pub(crate) struct GhAsset {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub size: u64,
}

/// La fiche d'une release (`release.json`, écrite par scripts/release-manifest.mjs).
#[derive(Deserialize, Clone, Debug)]
pub struct ReleaseFile {
    pub name: String,
    pub version: Version,
    #[serde(default)]
    pub(crate) requires: ReleaseRequires,
    #[serde(default)]
    pub(crate) assets: Vec<ReleaseAsset>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub(crate) struct ReleaseRequires {
    pub core: Option<VersionReq>,
}

#[derive(Deserialize, Clone, Debug)]
pub(crate) struct ReleaseAsset {
    pub name: String,
    #[serde(default)]
    pub kind: String,
    pub sha256: String,
}

/// Un client sans limite de durée totale (les archives sont grosses), mais qui n'attend pas un
/// serveur muet ; les en-têtes d'autorisation ne suivent pas une redirection (le stockage de GitHub).
pub(crate) fn agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .max_redirects(5)
        .http_status_as_error(false)
        .user_agent(format!("strata-core/{}", strata_core::VERSION))
        .build()
        .into()
}

pub(crate) fn gh_get(agent: &ureq::Agent, url: &str, repo: &str, key: &str, accept: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    let mut req = agent.get(url).header("Accept", accept).header("X-GitHub-Api-Version", "2022-11-28");
    if !key.is_empty() {
        req = req.header("Authorization", &format!("Bearer {key}"));
    }
    let res = req.call().map_err(|e| format!("cannot reach GitHub: {e}"))?;
    match res.status().as_u16() {
        200..=299 => Ok(res),
        401 => Err(format!("GitHub refused the key of {repo}")),
        403 | 429 => Err("GitHub refused the request (too many requests, or the key lacks the rights)".into()),
        404 => Err(format!("{repo} was not found on GitHub (a private repository needs a key)")),
        c => Err(format!("GitHub answered HTTP {c}")),
    }
}

pub(crate) fn asset_url(api: &str, repo: &str, id: u64) -> String {
    format!("{api}/repos/{repo}/releases/assets/{id}")
}

/// La release à installer : la plus récente (hors brouillons et préversions) dont la fiche accepte ce
/// Core.
pub(crate) fn pick(agent: &ureq::Agent, api: &str, repo: &str, key: &str) -> Result<(GhRelease, ReleaseFile), String> {
    let core = Version::parse(strata_core::VERSION).expect("the Core's version");
    let mut res = gh_get(agent, &format!("{api}/repos/{repo}/releases?per_page=30"), repo, key, "application/vnd.github+json")?;
    let bytes = res.body_mut().with_config().limit(16 << 20).read_to_vec().map_err(|e| format!("GitHub's answer is unreadable: {e}"))?;
    let list: Vec<GhRelease> = serde_json::from_slice(&bytes).map_err(|e| format!("GitHub's answer is unreadable: {e}"))?;
    let mut refused: Option<(Version, VersionReq)> = None;
    for r in list.into_iter().filter(|r| !r.draft && !r.prerelease) {
        let Some(a) = r.assets.iter().find(|a| a.name == "release.json") else { continue };
        let mut res = gh_get(agent, &asset_url(api, repo, a.id), repo, key, "application/octet-stream")?;
        let bytes = res.body_mut().with_config().limit(1 << 20).read_to_vec().map_err(|e| format!("cannot read release.json of {}: {e}", r.tag_name))?;
        let file: ReleaseFile = serde_json::from_slice(&bytes).map_err(|e| format!("release.json of {} is unreadable: {e}", r.tag_name))?;
        match &file.requires.core {
            Some(req) if !accepts(req, &core) => {
                refused.get_or_insert((file.version.clone(), req.clone()));
            }
            _ => return Ok((r, file)),
        }
    }
    match refused {
        Some((v, req)) => Err(format!("no release of {repo} accepts the Core {core} (the latest, {v}, needs {req})")),
        None => Err(format!("{repo} has no Strata release")),
    }
}

/// L'archive de l'appli dans une release.
fn archive_of(file: &ReleaseFile) -> Option<&ReleaseAsset> {
    file.assets
        .iter()
        .find(|a| a.kind == "app-linux-x86_64")
        .or_else(|| file.assets.iter().find(|a| a.name.ends_with("-linux-x86_64.tar.gz") && !a.name.starts_with("strata-core-")))
}

/// Télécharge vers `to` en calculant l'empreinte ; `progress(fait, total)`.
pub(crate) fn download(agent: &ureq::Agent, url: &str, repo: &str, key: &str, to: &Path, progress: impl Fn(u64, u64)) -> Result<String, String> {
    let res = gh_get(agent, url, repo, key, "application/octet-stream")?;
    let total = res.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0);
    if total > MAX_ARCHIVE {
        return Err("the archive is too big".into());
    }
    let mut body = res.into_body().into_reader();
    let mut out = std::fs::File::create(to).map_err(|e| format!("cannot write {}: {e}", to.display()))?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut done = 0u64;
    loop {
        let n = body.read(&mut buf).map_err(|e| format!("the download was cut: {e}"))?;
        if n == 0 {
            break;
        }
        done += n as u64;
        if done > MAX_ARCHIVE {
            return Err("the archive is too big".into());
        }
        hash.update(&buf[..n]);
        out.write_all(&buf[..n]).map_err(|e| format!("cannot write {}: {e}", to.display()))?;
        progress(done, total.max(done));
    }
    out.sync_all().map_err(|e| e.to_string())?;
    Ok(hex(&hash.finalize()))
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ---------- L'archive ----------

/// Une appli dépaquetée à côté, pas encore en place. Ce qui reste du dossier est effacé à la fin.
pub struct Staged {
    staging: PathBuf,
    root: PathBuf,
    pub manifest: AppManifest,
}

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.staging);
    }
}

/// Dépaquette une archive d'appli dans `apps/.staging-…` et lit sa fiche.
pub fn unpack(archive: &Path, apps_dir: &Path) -> Result<Staged, String> {
    let staging = apps_dir.join(format!(".staging-{}", strata_core::new_id()));
    std::fs::create_dir_all(&staging).map_err(|e| format!("cannot create {}: {e}", staging.display()))?;
    let mut st = Staged { root: staging.clone(), staging, manifest: placeholder() };
    let mut f = std::fs::File::open(archive).map_err(|e| format!("cannot open the archive: {e}"))?;
    let mut magic = [0u8; 2];
    if f.read_exact(&mut magic).is_err() || magic != [0x1f, 0x8b] {
        return Err("this is not a .tar.gz archive".into());
    }
    std::io::Seek::rewind(&mut f).map_err(|e| e.to_string())?;
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(BufReader::new(f)));
    let mut total = 0u64;
    let entries = ar.entries().map_err(|_| "this is not a .tar.gz archive".to_string())?;
    for e in entries {
        let mut e = e.map_err(|_| "the archive is damaged".to_string())?;
        let kind = e.header().entry_type();
        if !(kind.is_file() || kind.is_dir() || kind.is_symlink()) {
            return Err("the archive holds unexpected entries (links, devices)".into());
        }
        total += e.size();
        if total > MAX_UNPACKED {
            return Err("the archive is too big".into());
        }
        // Un lien ne pointe que dans l'archive (pas de chemin absolu, pas de `..`).
        if kind.is_symlink() {
            let target = e.link_name().ok().flatten().map(|t| t.into_owned()).unwrap_or_default();
            let inside = target.is_relative() && !target.components().any(|c| matches!(c, std::path::Component::ParentDir));
            if !inside {
                return Err(format!("the archive holds a link outside of it: {}", target.display()));
            }
        }
        // Ni setuid/setgid, ni fichier modifiable par d'autres comptes.
        e.set_mask(0o6022);
        // `unpack_in` refuse les chemins qui sortiraient du dossier.
        e.unpack_in(&st.staging).map_err(|err| format!("cannot unpack the archive: {err}"))?;
    }
    // La fiche à la racine, ou dans l'unique dossier de l'archive (`livecut-0.3.0/`).
    if !st.staging.join("strata-app.toml").exists() {
        let dirs: Vec<PathBuf> = std::fs::read_dir(&st.staging).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
        match dirs.as_slice() {
            [one] if one.is_dir() && one.join("strata-app.toml").exists() => st.root = one.clone(),
            _ => return Err("the archive has no strata-app.toml".into()),
        }
    }
    let text = std::fs::read_to_string(st.root.join("strata-app.toml")).map_err(|e| format!("cannot read strata-app.toml: {e}"))?;
    st.manifest = AppManifest::parse(&text, "strata-app.toml").map_err(|e| e.to_string())?;
    Ok(st)
}

fn placeholder() -> AppManifest {
    AppManifest::parse("id = \"pending\"\nname = \"-\"\nversion = \"0.0.0\"\nport = 1024\n[requires]\ncore = \"*\"\n", "-").expect("placeholder")
}

/// Met l'appli dépaquetée en place (à la place de l'ancienne version) et retient d'où elle vient.
fn commit(s: &Shared, st: Staged, source: Option<Source>, release: Option<&ReleaseFile>) -> Result<Installed, String> {
    let m = st.manifest.clone();
    if let Some(r) = release {
        if r.name != m.id || r.version != m.version {
            return Err(format!("the release is {} {} but its archive holds {} {}", r.name, r.version, m.id, m.version));
        }
    }
    let core = Version::parse(strata_core::VERSION).expect("the Core's version");
    if !m.accepts(&core) {
        return Err(format!("{} {} needs the Core {} (this one is {})", m.name, m.version, m.requires.core, strata_core::VERSION));
    }
    let _one = s.installer.swap.lock().unwrap();
    if let Some(o) = s.apps.list().into_iter().find(|a| a.manifest.id != m.id && a.manifest.port == m.port) {
        return Err(format!("{} already uses port {}", o.manifest.name, m.port));
    }
    if let (Some(new), Some(old)) = (&source, s.installer.source(&m.id)) {
        if !new.repo.eq_ignore_ascii_case(&old.repo) {
            return Err(format!("{} is already installed from {}", m.name, old.repo));
        }
    }
    let apps_dir = s.apps.dir().to_path_buf();
    let dest = apps_dir.join(&m.id);
    let old = if dest.exists() {
        let o = apps_dir.join(format!(".old-{}-{}", m.id, strata_core::new_id()));
        std::fs::rename(&dest, &o).map_err(|e| format!("cannot move the installed version: {e}"))?;
        Some(o)
    } else {
        None
    };
    if let Err(e) = std::fs::rename(&st.root, &dest) {
        if let Some(o) = &old {
            let _ = std::fs::rename(o, &dest);
        }
        return Err(format!("cannot put the app in place: {e}"));
    }
    if let Some(o) = old {
        let _ = std::fs::remove_dir_all(o);
    }
    if let Some(src) = source {
        s.installer.set_source(&m.id, Some(src))?;
    }
    s.apps.forget();
    s.apps.get(&m.id).ok_or_else(|| format!("{} cannot be read after its install", m.id))
}

/// Relance une appli active après sa mise à jour (sa commande n'a pas changé : le Core ne la
/// relancerait pas de lui-même).
async fn restart_if_active(s: &Shared, app: &Installed) {
    if !(app.active && app.compatible) {
        return;
    }
    let cfg = s.config.load();
    if let Some(l) = launch_of(app, &s.data_dir, &cfg, s.core_port()) {
        s.supervisor.restart(&app.manifest.id, l).await;
    }
}

/// Lance l'installation (ou la mise à jour) depuis un dépôt GitHub, en tâche de fond.
/// `key` : None garde la clé connue pour ce dépôt ; `only` : l'appli attendue (mise à jour).
pub fn start(s: &Shared, repo: &str, key: Option<String>, only: Option<String>) -> Result<Job, String> {
    let repo = parse_repo(repo)?;
    let key = key.map(|k| k.trim().to_string()).unwrap_or_else(|| s.installer.key_of(&repo));
    let job = Job { repo: repo.clone(), step: Step::Looking, done: 0, total: 0, version: None, app: only.clone(), error: None, ended: None };
    {
        let mut jobs = s.installer.jobs.lock().unwrap();
        if jobs.get(&repo.to_lowercase()).is_some_and(|j| j.ended.is_none()) {
            return Err(format!("{repo} is already being installed"));
        }
        jobs.insert(repo.to_lowercase(), job.clone());
    }
    let s = s.clone();
    tokio::spawn(async move {
        let s2 = s.clone();
        let (r2, k2) = (repo.clone(), key.clone());
        let done = tokio::task::spawn_blocking(move || fetch_and_commit(&s2, &r2, &k2, only.as_deref())).await;
        let result = match done {
            Ok(r) => r,
            Err(e) => Err(e.to_string()),
        };
        match result {
            Ok(app) => {
                restart_if_active(&s, &app).await;
                tracing::info!("{} {} installed from {repo}", app.manifest.id, app.manifest.version);
                s.installer.job(&repo, |j| {
                    j.step = Step::Done;
                    j.app = Some(app.manifest.id.clone());
                    j.version = Some(app.manifest.version.to_string());
                });
            }
            Err(e) => {
                tracing::warn!("install from {repo} failed: {e}");
                s.installer.job(&repo, |j| {
                    j.step = Step::Failed;
                    j.error = Some(e);
                });
            }
        }
    });
    Ok(job)
}

/// Installe (ou met à jour) une appli depuis son dépôt, tout de suite (la ligne de commande,
/// l'installateur).
pub fn install_now(s: &Shared, repo: &str, key: Option<&str>) -> Result<Installed, String> {
    let repo = parse_repo(repo)?;
    let key = key.map(|k| k.trim().to_string()).unwrap_or_else(|| s.installer.key_of(&repo));
    fetch_and_commit(s, &repo, &key, None)
}

/// Installe une appli depuis son archive `.tar.gz` (serveur sans internet).
pub fn install_archive(s: &Shared, archive: &Path) -> Result<Installed, String> {
    let apps_dir = s.apps.dir().to_path_buf();
    std::fs::create_dir_all(&apps_dir).map_err(|e| e.to_string())?;
    let st = unpack(archive, &apps_dir)?;
    commit(s, st, None, None)
}

fn fetch_and_commit(s: &Shared, repo: &str, key: &str, only: Option<&str>) -> Result<Installed, String> {
    let agent = agent();
    let api = s.installer.api();
    let (release, file) = pick(&agent, &api, repo, key)?;
    if let Some(id) = only {
        if file.name != id {
            return Err(format!("{repo} now publishes {} instead of {id}", file.name));
        }
    }
    let wanted = archive_of(&file).ok_or_else(|| format!("release {} has no app archive", release.tag_name))?;
    let asset = release.assets.iter().find(|a| a.name == wanted.name).ok_or_else(|| format!("{} is missing from release {}", wanted.name, release.tag_name))?;
    if asset.size > MAX_ARCHIVE {
        return Err("the archive is too big".into());
    }
    s.installer.job(repo, |j| {
        j.step = Step::Downloading;
        j.version = Some(file.version.to_string());
        j.app = Some(file.name.clone());
        j.total = asset.size;
    });
    let apps_dir = s.apps.dir().to_path_buf();
    std::fs::create_dir_all(&apps_dir).map_err(|e| e.to_string())?;
    let tmp = TempFile(apps_dir.join(format!(".download-{}.tar.gz", strata_core::new_id())));
    let sum = download(&agent, &asset_url(&api, repo, asset.id), repo, key, &tmp.0, |done, total| {
        s.installer.job(repo, |j| {
            j.done = done;
            j.total = total;
        })
    })?;
    if !sum.eq_ignore_ascii_case(wanted.sha256.trim()) {
        return Err("the download does not match its fingerprint".into());
    }
    s.installer.job(repo, |j| j.step = Step::Installing);
    let st = unpack(&tmp.0, &apps_dir)?;
    commit(s, st, Some(Source { repo: repo.to_string(), key: key.to_string() }), Some(&file))
}

/// Un fichier effacé quand on en a fini.
pub(crate) struct TempFile(pub PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Désinstalle une appli inactive ; `data` : efface aussi ses données.
pub fn uninstall(s: &Shared, id: &str, data: bool) -> Result<Installed, String> {
    let app = s.apps.get(id).ok_or_else(|| format!("no such app: {id}"))?;
    if app.active {
        return Err(format!("{} is active: deactivate it first", app.manifest.name));
    }
    let _one = s.installer.swap.lock().unwrap();
    let apps_dir = s.apps.dir();
    let trash = apps_dir.join(format!(".old-{id}-{}", strata_core::new_id()));
    std::fs::rename(&app.dir, &trash).map_err(|e| format!("cannot remove {}: {e}", app.manifest.name))?;
    let _ = std::fs::remove_dir_all(&trash);
    s.installer.set_source(id, None)?;
    if data {
        let d = s.data_dir.join("app-data").join(id);
        if d.exists() {
            std::fs::remove_dir_all(&d).map_err(|e| format!("cannot remove the data of {}: {e}", app.manifest.name))?;
        }
    }
    s.apps.forget();
    Ok(app)
}

// ---------- API (administrateurs) ----------

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/apps/install", post(install))
        .route("/api/apps/upload", post(upload).layer(DefaultBodyLimit::disable()))
        .route("/api/apps/jobs/dismiss", post(dismiss))
        .route("/api/apps/{id}/update", get(check_update).post(update))
        .route("/api/apps/{id}/key", put(set_key))
        .route("/api/apps/{id}", delete(remove))
}

#[derive(Deserialize)]
struct InstallBody {
    repo: String,
    #[serde(default)]
    key: Option<String>,
}

async fn install(State(s): State<Shared>, CurrentUser(me): CurrentUser, Json(b): Json<InstallBody>) -> ApiResult<(StatusCode, Json<Job>)> {
    require(me.admin)?;
    let job = start(&s, &b.repo, b.key, None).map_err(bad)?;
    tracing::info!("install from {} asked by {}", job.repo, me.username);
    Ok((StatusCode::ACCEPTED, Json(job)))
}

#[derive(Deserialize)]
struct DismissBody {
    repo: String,
}

async fn dismiss(State(s): State<Shared>, CurrentUser(me): CurrentUser, Json(b): Json<DismissBody>) -> ApiResult<StatusCode> {
    require(me.admin)?;
    s.installer.dismiss(&b.repo);
    Ok(StatusCode::NO_CONTENT)
}

/// Une archive envoyée par le navigateur (serveur sans internet).
async fn upload(State(s): State<Shared>, CurrentUser(me): CurrentUser, headers: HeaderMap, mut body: Body) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let apps_dir = s.apps.dir().to_path_buf();
    tokio::fs::create_dir_all(&apps_dir).await.map_err(crate::api::internal)?;
    let tmp = TempFile(apps_dir.join(format!(".upload-{}.tar.gz", strata_core::new_id())));
    {
        use tokio::io::AsyncWriteExt;
        let mut f = tokio::fs::File::create(&tmp.0).await.map_err(crate::api::internal)?;
        let mut size = 0u64;
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|e| bad(format!("the upload was cut: {e}")))?;
            if let Ok(data) = frame.into_data() {
                size += data.len() as u64;
                if size > MAX_ARCHIVE {
                    return Err((StatusCode::PAYLOAD_TOO_LARGE, "the archive is too big".into()));
                }
                f.write_all(&data).await.map_err(crate::api::internal)?;
            }
        }
        f.flush().await.map_err(crate::api::internal)?;
    }
    let s2 = s.clone();
    let app = blocking(move || {
        let st = unpack(&tmp.0, &apps_dir)?;
        commit(&s2, st, None, None)
    })
    .await?
    .map_err(bad)?;
    restart_if_active(&s, &app).await;
    tracing::info!("{} {} installed from an archive by {}", app.manifest.id, app.manifest.version, me.username);
    Ok(Json(app_json(&s, &app, &s.config.load(), host(&headers)).await))
}

/// Y a-t-il une version plus récente, qui accepte ce Core ?
async fn check_update(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath(id): UrlPath<String>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let app = s.apps.get(&id).ok_or((StatusCode::NOT_FOUND, format!("no such app: {id}")))?;
    let src = s.installer.source(&id).ok_or_else(|| bad(format!("{} was not installed from GitHub", app.manifest.name)))?;
    let api = s.installer.api();
    let (_, file) = blocking(move || pick(&agent(), &api, &src.repo, &src.key)).await?.map_err(bad)?;
    Ok(Json(json!({
        "current": app.manifest.version.to_string(),
        "latest": file.version.to_string(),
        "newer": file.version > app.manifest.version && file.name == id,
    })))
}

async fn update(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath(id): UrlPath<String>) -> ApiResult<(StatusCode, Json<Job>)> {
    require(me.admin)?;
    let app = s.apps.get(&id).ok_or((StatusCode::NOT_FOUND, format!("no such app: {id}")))?;
    let src = s.installer.source(&id).ok_or_else(|| bad(format!("{} was not installed from GitHub", app.manifest.name)))?;
    let job = start(&s, &src.repo, None, Some(id)).map_err(bad)?;
    tracing::info!("update of {} asked by {}", app.manifest.id, me.username);
    Ok((StatusCode::ACCEPTED, Json(job)))
}

#[derive(Deserialize)]
struct KeyBody {
    key: String,
}

/// Change la clé du dépôt d'une appli (vide : aucune).
async fn set_key(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath(id): UrlPath<String>, Json(b): Json<KeyBody>) -> ApiResult<StatusCode> {
    require(me.admin)?;
    let mut src = s.installer.source(&id).ok_or_else(|| bad(format!("{id} was not installed from GitHub")))?;
    src.key = b.key.trim().to_string();
    s.installer.set_source(&id, Some(src)).map_err(crate::api::internal)?;
    tracing::info!("key of {id} changed by {}", me.username);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct RemoveQuery {
    #[serde(default)]
    data: bool,
}

async fn remove(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath(id): UrlPath<String>, Query(q): Query<RemoveQuery>) -> ApiResult<StatusCode> {
    require(me.admin)?;
    let s2 = s.clone();
    let id2 = id.clone();
    let app =
        blocking(move || uninstall(&s2, &id2, q.data)).await?.map_err(|e| if e.starts_with("no such app") { (StatusCode::NOT_FOUND, e) } else { bad(e) })?;
    tracing::info!("{} uninstalled by {}{}", app.manifest.id, me.username, if q.data { " (data removed)" } else { "" });
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repositories() {
        assert_eq!(parse_repo("DamienP2/Strata-Cut").unwrap(), "DamienP2/Strata-Cut");
        assert_eq!(parse_repo(" https://github.com/DamienP2/Strata-Cut.git/ ").unwrap(), "DamienP2/Strata-Cut");
        assert_eq!(parse_repo("github.com/a/b").unwrap(), "a/b");
        assert!(parse_repo("strata-cut").is_err());
        assert!(parse_repo("a/b/c").is_err());
        assert!(parse_repo("../b").is_err());
        assert!(parse_repo("a/b?x=1").is_err());
    }
}
