//! Les applis du serveur et le cockpit.
//!
//! Une appli installée a son dossier `<données>/apps/<id>/` avec sa fiche `strata-app.toml` ;
//! `<données>/apps.json` dit lesquelles sont actives. Le cockpit montre les applis actives,
//! compatibles avec ce Core, une tuile par appli.
use axum::{
    extract::{Path as UrlPath, State},
    routing::{get, patch},
    Json, Router,
};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use strata_core::manifest::AppManifest;

use crate::{
    api::{bad, blocking, require, ApiResult},
    auth::CurrentUser,
    config::{hostname, ServerConfig},
    state::Shared,
};

/// Une appli installée sur ce serveur.
#[derive(Clone, Debug)]
pub struct Installed {
    pub manifest: AppManifest,
    pub active: bool,
    /// Accepte-t-elle ce Core ?
    pub compatible: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct ActiveFile {
    #[serde(default)]
    active: Vec<String>,
}

pub struct Apps {
    dir: PathBuf,
    state: PathBuf,
}

impl Apps {
    pub fn new(data_dir: &Path) -> Self {
        Apps { dir: data_dir.join("apps"), state: data_dir.join("apps.json") }
    }

    fn active_ids(&self) -> Vec<String> {
        strata_core::files::read_json::<ActiveFile>(&self.state).unwrap_or_default().active
    }

    /// Les applis installées, par nom ; une fiche illisible est écartée (et dite au journal).
    pub fn list(&self) -> Vec<Installed> {
        let core = Version::parse(strata_core::VERSION).expect("the Core's version");
        let active = self.active_ids();
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return out };
        for e in entries.flatten() {
            if !e.path().is_dir() {
                continue;
            }
            match AppManifest::read(&e.path()) {
                Ok(m) => {
                    let compatible = m.accepts(&core);
                    out.push(Installed { active: active.contains(&m.id), compatible, manifest: m });
                }
                Err(err) => tracing::warn!("{err}"),
            }
        }
        out.sort_by_key(|a| a.manifest.name.to_lowercase());
        out
    }

    pub fn get(&self, id: &str) -> Option<Installed> {
        self.list().into_iter().find(|a| a.manifest.id == id)
    }

    /// Active ou désactive une appli installée. Une appli qui n'accepte pas ce Core ne s'active pas.
    pub fn set_active(&self, id: &str, on: bool) -> Result<Installed, String> {
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
        let mut ids = self.active_ids();
        ids.retain(|x| x != id);
        if on {
            ids.push(id.to_string());
        }
        ids.sort();
        strata_core::files::write_json(&self.state, &ActiveFile { active: ids }, false).map_err(|e| format!("cannot save the apps: {e}"))?;
        Ok(Installed { active: on, ..app })
    }
}

pub fn routes() -> Router<Shared> {
    Router::new().route("/api/cockpit", get(cockpit)).route("/api/apps", get(list)).route("/api/apps/{id}", patch(update))
}

/// Une appli telle que les réglages la montrent.
fn app_json(a: &Installed, cfg: &ServerConfig) -> Value {
    let m = &a.manifest;
    json!({
        "id": m.id,
        "name": m.name,
        "version": m.version.to_string(),
        "icon": m.icon,
        "port": m.port,
        "requires": m.requires.core.to_string(),
        "compatible": a.compatible,
        "active": a.active,
        "url": cfg.app_url(&m.id),
    })
}

/// La Bibliothèque : les applis installées sur ce serveur (administrateurs).
async fn list(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let cfg = s.config.load();
    Ok(Json(json!({
        "core": strata_core::VERSION,
        "apps": s.apps.list().iter().map(|a| app_json(a, &cfg)).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
struct Patch {
    active: Option<bool>,
}

/// Active ou désactive une appli (administrateurs).
async fn update(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath(id): UrlPath<String>, Json(b): Json<Patch>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let Some(on) = b.active else { return Err(bad("nothing to change")) };
    let s2 = s.clone();
    let app = blocking(move || s2.apps.set_active(&id, on)).await?.map_err(|e| {
        if e.starts_with("no such app") {
            (axum::http::StatusCode::NOT_FOUND, e)
        } else {
            bad(e)
        }
    })?;
    tracing::info!("{} {} by {}", app.manifest.id, if on { "activated" } else { "deactivated" }, me.username);
    Ok(Json(app_json(&app, &s.config.load())))
}

/// Ce que montre le cockpit (`CockpitData` de @strata/ui) : les applis actives, ce serveur, une
/// instance par appli. Plusieurs serveurs (maître et membres) : brief § 4 bis, à venir.
///
/// Les droits sont gérés dans chaque appli : le Core leur demandera « cet utilisateur a-t-il accès ? »
/// quand elles sauront répondre ; d'ici là, toute appli active est montrée à tout utilisateur connecté.
async fn cockpit(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> Json<Value> {
    let cfg = s.config.load();
    let server = hostname();
    let apps: Vec<Installed> = s.apps.list().into_iter().filter(|a| a.active && a.compatible).collect();
    Json(json!({
        "admin": me.admin,
        "apps": apps.iter().map(|a| json!({"id": a.manifest.id, "name": a.manifest.name, "icon": a.manifest.icon})).collect::<Vec<_>>(),
        "servers": [{"id": server, "name": cfg.name, "host": server, "up": true, "alerts": 0, "master": true}],
        "instances": apps.iter().map(|a| json!({"app": a.manifest.id, "server": server, "url": cfg.app_url(&a.manifest.id)})).collect::<Vec<_>>(),
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
        assert!(apps.set_active("graphics", true).unwrap_err().contains("needs the Core"));
        assert!(apps.set_active("ghost", true).is_err());
        assert!(apps.get("livecut").unwrap().active);
        apps.set_active("livecut", false).unwrap();
        assert!(!apps.get("livecut").unwrap().active);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
