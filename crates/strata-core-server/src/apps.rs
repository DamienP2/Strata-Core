//! Les applis du serveur et le cockpit.
//!
//! Une appli installée a son dossier `<données>/apps/<id>/` avec sa fiche `strata-app.toml` ;
//! `<données>/apps.json` dit lesquelles sont actives. Le cockpit montre les applis actives,
//! compatibles avec ce Core, une tuile par appli.
use axum::{
    extract::{Path as UrlPath, State},
    http::{HeaderMap, StatusCode},
    routing::{get, patch, post},
    Json, Router,
};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};
use strata_core::manifest::AppManifest;

use crate::{
    api::{bad, blocking, require, ApiResult},
    auth::{host, CurrentUser},
    config::{hostname, ServerConfig},
    state::Shared,
    supervisor::{answers, Launch},
};

/// Une appli installée sur ce serveur.
#[derive(Clone, Debug)]
pub struct Installed {
    pub manifest: AppManifest,
    /// Son dossier (`<données>/apps/<id>/`).
    pub dir: PathBuf,
    pub active: bool,
    /// Accepte-t-elle ce Core ?
    pub compatible: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct ActiveFile {
    #[serde(default)]
    active: Vec<String>,
}

/// Combien de temps la liste lue sur le disque sert (le relais la consulte à chaque requête). Elle
/// est relue aussitôt quand le Core la change, ou quand `apps.json` ou le dossier `apps/` changent
/// (commande `app`) ; une fiche modifiée à la main se voit au plus tard après ce délai.
const FRESH: Duration = Duration::from_secs(2);

type Stamp = (Option<SystemTime>, Option<SystemTime>);
/// La liste lue, quand, et l'état des fichiers à ce moment.
type Cached = (Instant, Stamp, Arc<Vec<Installed>>);

pub struct Apps {
    dir: PathBuf,
    state: PathBuf,
    cache: Mutex<Option<Cached>>,
}

impl Apps {
    pub fn new(data_dir: &Path) -> Self {
        Apps { dir: data_dir.join("apps"), state: data_dir.join("apps.json"), cache: Mutex::new(None) }
    }

    fn active_ids(&self) -> Vec<String> {
        strata_core::files::read_json::<ActiveFile>(&self.state).unwrap_or_default().active
    }

    /// Les applis installées, par nom ; une fiche illisible est écartée (et dite au journal).
    pub fn list(&self) -> Vec<Installed> {
        self.all().as_ref().clone()
    }

    fn all(&self) -> Arc<Vec<Installed>> {
        let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        let stamp = (modified(&self.state), modified(&self.dir));
        let mut cache = self.cache.lock().unwrap();
        if let Some((at, was, list)) = cache.as_ref() {
            if at.elapsed() < FRESH && *was == stamp {
                return list.clone();
            }
        }
        let list = Arc::new(self.read());
        *cache = Some((Instant::now(), stamp, list.clone()));
        list
    }

    /// Le dossier des applis installées.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub(crate) fn forget(&self) {
        *self.cache.lock().unwrap() = None;
    }

    fn read(&self) -> Vec<Installed> {
        let core = Version::parse(strata_core::VERSION).expect("the Core's version");
        let active = self.active_ids();
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return out };
        for e in entries.flatten() {
            // `.staging-…`, `.old-…` : une installation en cours ou une version qu'on efface.
            if !e.path().is_dir() || e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            match AppManifest::read(&e.path()) {
                Ok(m) => {
                    let compatible = m.accepts(&core);
                    out.push(Installed { active: active.contains(&m.id), compatible, manifest: m, dir: e.path() });
                }
                Err(err) => tracing::warn!("{err}"),
            }
        }
        out.sort_by_key(|a| a.manifest.name.to_lowercase());
        out
    }

    pub fn get(&self, id: &str) -> Option<Installed> {
        self.all().iter().find(|a| a.manifest.id == id).cloned()
    }

    /// Active ou désactive une appli installée. Une appli qui n'accepte pas ce Core ne s'active pas.
    pub fn set_active(&self, id: &str, on: bool) -> Result<Installed, String> {
        self.forget();
        let app = self.get(id).ok_or_else(|| format!("no such app: {id}"))?;
        if on && !app.compatible {
            return Err(format!(
                "{} {} needs the Core {} (this one is {})",
                app.manifest.name,
                app.manifest.version,
                app.manifest.requires.core,
                strata_core::VERSION
            ));
        }
        if on {
            let port = app.manifest.port;
            if let Some(other) = self.all().iter().find(|a| a.active && a.manifest.id != id && a.manifest.port == port) {
                return Err(format!("{} uses port {port} too: deactivate it first", other.manifest.name));
            }
        }
        let mut ids = self.active_ids();
        ids.retain(|x| x != id);
        if on {
            ids.push(id.to_string());
        }
        ids.sort();
        strata_core::files::write_json(&self.state, &ActiveFile { active: ids }, false).map_err(|e| format!("cannot save the apps: {e}"))?;
        self.forget();
        Ok(Installed { active: on, ..app })
    }
}

/// Comment le Core lance une appli : sa commande `[run]`, dans son dossier, avec les variables
/// `STRATA_*` (docs/APPLIS.md). Aucune sans `[run]` : elle est lancée à part.
pub fn launch_of(a: &Installed, data_dir: &Path, cfg: &ServerConfig, core_port: u16) -> Option<Launch> {
    let run = a.manifest.run.as_ref()?;
    let m = &a.manifest;
    let program = if Path::new(&run.command).is_absolute() { PathBuf::from(&run.command) } else { a.dir.join(&run.command) };
    let data = data_dir.join("app-data").join(&m.id);
    if let Err(e) = std::fs::create_dir_all(&data) {
        tracing::warn!("cannot create {}: {e}", data.display());
    }
    let env = [
        ("STRATA_APP_ID", m.id.clone()),
        ("STRATA_APP_VERSION", m.version.to_string()),
        ("STRATA_APP_PORT", m.port.to_string()),
        ("STRATA_APP_BIND", format!("127.0.0.1:{}", m.port)),
        ("STRATA_APP_URL", cfg.app_url(&m.id)),
        ("STRATA_APP_DATA", data.display().to_string()),
        ("STRATA_CORE_URL", format!("http://127.0.0.1:{core_port}")),
        ("STRATA_CORE_VERSION", strata_core::VERSION.to_string()),
        ("STRATA_SERVER_NAME", cfg.name.clone()),
        // Les réglages réseau de NDI, les mêmes pour toutes les applis (Matériel › NDI).
        ("NDI_CONFIG_DIR", crate::hardware::ndi::config_dir(data_dir).display().to_string()),
    ];
    Some(Launch { program, args: run.args.clone(), cwd: a.dir.clone(), env: env.into_iter().map(|(k, v)| (k.to_string(), v)).collect(), port: m.port })
}

/// Met les applis lancées en accord avec les applis actives : lance celles qui doivent tourner,
/// arrête les autres. Appelé à chaque changement et régulièrement (une activation faite à côté).
pub async fn reconcile(s: &Shared) {
    let cfg = s.config.load();
    let want: Vec<(String, Launch)> = s
        .apps
        .list()
        .iter()
        .filter(|a| a.active && a.compatible)
        .filter_map(|a| launch_of(a, &s.data_dir, &cfg, s.core_port()).map(|l| (a.manifest.id.clone(), l)))
        .collect();
    for id in s.supervisor.running() {
        if !want.iter().any(|(w, _)| *w == id) {
            s.supervisor.stop(&id).await;
        }
    }
    for (id, l) in want {
        s.supervisor.start(&id, l).await;
    }
}

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/cockpit", get(cockpit))
        .route("/api/apps", get(list))
        .route("/api/apps/{id}", patch(update))
        .route("/api/apps/{id}/restart", post(restart))
        .route("/api/apps/{id}/log", get(log))
}

/// Une appli telle que les réglages la montrent, avec son service : lancé par le Core
/// (`managed`, son état) ou à part, et si son port répond.
pub async fn app_json(s: &Shared, a: &Installed, cfg: &ServerConfig, at: Option<&str>) -> Value {
    let m = &a.manifest;
    let mut run = serde_json::to_value(s.supervisor.status(&m.id)).unwrap_or_default();
    run["managed"] = json!(m.run.is_some());
    run["answers"] = json!(a.active && answers(m.port).await);
    json!({
        "id": m.id,
        "name": m.name,
        "version": m.version.to_string(),
        "icon": m.icon,
        "port": m.port,
        "requires": m.requires.core.to_string(),
        "compatible": a.compatible,
        "active": a.active,
        "url": cfg.app_url_at(&m.id, at),
        "public": m.relay.public,
        "run": run,
        // D'où elle vient (sans sa clé, jamais envoyée au navigateur).
        "source": s.installer.source(&m.id).map(|x| json!({"repo": x.repo, "key": !x.key.is_empty()})),
    })
}

fn not_found(e: String) -> (StatusCode, String) {
    if e.starts_with("no such app") {
        (StatusCode::NOT_FOUND, e)
    } else {
        bad(e)
    }
}

/// La Bibliothèque : les applis installées sur ce serveur (administrateurs).
async fn list(State(s): State<Shared>, CurrentUser(me): CurrentUser, headers: HeaderMap) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let cfg = s.config.load();
    let mut apps = Vec::new();
    for a in s.apps.list() {
        apps.push(app_json(&s, &a, &cfg, host(&headers)).await);
    }
    Ok(Json(json!({ "core": strata_core::VERSION, "apps": apps, "jobs": s.installer.jobs() })))
}

#[derive(Deserialize)]
struct Patch {
    active: Option<bool>,
}

/// Active ou désactive une appli (administrateurs).
async fn update(
    State(s): State<Shared>,
    CurrentUser(me): CurrentUser,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    Json(b): Json<Patch>,
) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let Some(on) = b.active else { return Err(bad("nothing to change")) };
    if on && s.apps.get(&id).is_some_and(|a| a.manifest.port == s.core_port()) {
        return Err(bad(format!("port {} is the Core's own", s.core_port())));
    }
    let s2 = s.clone();
    let app = blocking(move || s2.apps.set_active(&id, on)).await?.map_err(not_found)?;
    tracing::info!("{} {} by {}", app.manifest.id, if on { "activated" } else { "deactivated" }, me.username);
    reconcile(&s).await;
    Ok(Json(app_json(&s, &app, &s.config.load(), host(&headers)).await))
}

/// Relance une appli lancée par le Core (administrateurs).
async fn restart(State(s): State<Shared>, CurrentUser(me): CurrentUser, headers: HeaderMap, UrlPath(id): UrlPath<String>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let app = s.apps.get(&id).ok_or_else(|| not_found(format!("no such app: {id}")))?;
    if !app.active {
        return Err(bad(format!("{} is not active", app.manifest.name)));
    }
    let cfg = s.config.load();
    let launch = launch_of(&app, &s.data_dir, &cfg, s.core_port()).ok_or_else(|| bad(format!("{} is started apart from the Core", app.manifest.name)))?;
    tracing::info!("{} restarted by {}", app.manifest.id, me.username);
    s.supervisor.restart(&id, launch).await;
    Ok(Json(app_json(&s, &app, &cfg, host(&headers)).await))
}

/// Les dernières lignes du journal d'une appli lancée par le Core (administrateurs).
async fn log(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath(id): UrlPath<String>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    s.apps.get(&id).ok_or_else(|| not_found(format!("no such app: {id}")))?;
    Ok(Json(json!({ "lines": s.supervisor.log(&id) })))
}

/// Ce que montre le cockpit (`CockpitData` de @strata/ui) : les applis actives de ce serveur et,
/// sur un maître, celles de ses membres (brief § 4 bis) ; une tuile par appli, une instance par
/// serveur où elle tourne.
///
/// Les droits sont gérés dans chaque appli : le Core leur demande « cette personne a-t-elle accès ? »
/// (leur point `access`), ici et sur les membres ; une appli qui ne sait pas le dire est montrée.
async fn cockpit(State(s): State<Shared>, CurrentUser(me): CurrentUser, headers: HeaderMap) -> Json<Value> {
    let cfg = s.config.load();
    let me_id = s.cluster.id.clone();
    let who = crate::access::Who::of(&s, &me);
    let local: Vec<Installed> = s.apps.list().into_iter().filter(|a| a.active && a.compatible).collect();
    let (local, permitted) = tokio::join!(crate::access::filter(local, &who), crate::cluster::members_access(&s, &who));
    let mut apps: Vec<Value> = local.iter().map(|a| json!({"id": a.manifest.id, "name": a.manifest.name, "icon": a.manifest.icon})).collect();
    let mut instances = Vec::new();
    for a in &local {
        let up = answers(a.manifest.port).await;
        instances.push(json!({"app": a.manifest.id, "server": me_id, "url": cfg.app_url_at(&a.manifest.id, host(&headers)), "up": up}));
    }
    let mut servers = vec![
        json!({"id": me_id, "name": cfg.name, "host": hostname(), "up": true, "alerts": s.alerts.active().len(), "master": true, "url": cfg.core_url_at(host(&headers))}),
    ];
    // Les membres : leurs applis, d'après leur dernier battement.
    for (m, beat) in s.cluster.live_members() {
        let machine = m.url.split("://").nth(1).unwrap_or(&m.url).split(['/', ':']).next().unwrap_or_default().to_string();
        servers.push(json!({"id": m.id, "name": beat.as_ref().map_or(m.name.clone(), |b| b.name.clone()), "host": machine, "up": beat.is_some(), "alerts": beat.as_ref().map_or(0, |b| b.problems), "master": false, "url": m.url}));
        let allowed = permitted.get(&m.id).cloned().flatten();
        for a in beat.map(|b| b.apps).unwrap_or_default() {
            if allowed.as_ref().is_some_and(|ok| !ok.contains(&a.id)) {
                continue;
            }
            if !apps.iter().any(|x| x["id"] == a.id.as_str()) {
                apps.push(json!({"id": a.id, "name": a.name, "icon": a.icon}));
            }
            instances.push(json!({"app": a.id, "server": m.id, "url": a.url, "up": a.up}));
        }
    }
    Json(json!({
        "admin": me.admin,
        "apps": apps,
        "servers": servers,
        "instances": instances,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn install(data_dir: &Path, id: &str, name: &str, core: &str) {
        let dir = data_dir.join("apps").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("strata-app.toml"),
            format!("id = \"{id}\"\nname = \"{name}\"\nversion = \"1.0.0\"\nicon = \"scissors\"\nport = 8101\n\n[requires]\ncore = \"{core}\"\n"),
        )
        .unwrap();
    }

    #[test]
    fn installed_and_active() {
        let dir = std::env::temp_dir().join(format!("strata-apps-{}", strata_core::new_id()));
        install(&dir, "livecut", "LiveCut", ">=0.1, <1");
        install(&dir, "graphics", "Graphics", ">=9");
        std::fs::create_dir_all(dir.join("apps/broken")).unwrap();
        let apps = Apps::new(&dir);
        let list = apps.list();
        assert_eq!(list.iter().map(|a| a.manifest.id.as_str()).collect::<Vec<_>>(), ["graphics", "livecut"], "sorted, broken one left out");
        assert!(list.iter().all(|a| !a.active));
        assert!(apps.set_active("livecut", true).unwrap().active);
        install(&dir, "twin", "Twin", ">=0.1");
        assert!(apps.set_active("twin", true).unwrap_err().contains("LiveCut uses port 8101"), "one port, one app");
        assert!(apps.set_active("graphics", true).unwrap_err().contains("needs the Core"));
        assert!(apps.set_active("ghost", true).is_err());
        assert!(apps.get("livecut").unwrap().active);
        apps.set_active("livecut", false).unwrap();
        assert!(!apps.get("livecut").unwrap().active);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
