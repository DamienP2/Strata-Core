//! Le catalogue des applis Strata (Bibliothèque) : les dépôts connus (`catalog.json`, dans le Core),
//! et pour chacun ce que GitHub en dit — sa description, sa dernière release qui accepte ce Core —
//! et s'il est déjà installé ici. L'installation passe par l'installateur habituel.
//!
//! GitHub limite les appels sans clé (60 par heure) : les réponses sont gardées une heure, et la
//! clé d'une appli installée (ou celle du dépôt du Core) sert quand il y en a une.
use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use crate::{
    api::{blocking, require, ApiResult},
    auth::CurrentUser,
    install::{agent, gh_get, pick},
    state::Shared,
};

const KEEP: Duration = Duration::from_secs(3600);

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Entry {
    pub repo: String,
    pub name: String,
    #[serde(default)]
    pub icon: String,
    /// Une phrase, si le dépôt n'a pas de description sur GitHub.
    #[serde(default)]
    pub summary: String,
}

pub fn entries() -> Vec<Entry> {
    serde_json::from_str(include_str!("../catalog.json")).expect("catalog.json")
}

/// Ce que GitHub dit d'un dépôt.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
struct Remote {
    description: String,
    /// L'identifiant de l'appli (release.json) et sa dernière version qui accepte ce Core.
    app: Option<String>,
    version: Option<String>,
    /// Pourquoi il n'y a rien à installer (pas encore de release Strata, Core trop ancien…).
    error: Option<String>,
}

/// (quand, ce que GitHub a dit de chaque dépôt).
type Looked = (Instant, Vec<(String, Remote)>);
static CACHE: Mutex<Option<Looked>> = Mutex::new(None);

fn look(api: &str, repo: &str, key: &str) -> Remote {
    let a = agent();
    let description = gh_get(&a, &format!("{api}/repos/{repo}"), repo, key, "application/vnd.github+json")
        .ok()
        .and_then(|mut r| r.body_mut().with_config().limit(1 << 20).read_to_vec().ok())
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v["description"].as_str().map(String::from))
        .unwrap_or_default();
    match pick(&a, api, repo, key) {
        Ok((_, file)) => Remote { description, app: Some(file.name), version: Some(file.version.to_string()), error: None },
        Err(e) => Remote { description, error: Some(e), ..Default::default() },
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Ask {
    /// Relire GitHub, sans attendre l'heure.
    refresh: bool,
}

async fn index(State(s): State<Shared>, CurrentUser(me): CurrentUser, Query(q): Query<Ask>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let list = entries();
    let cached = CACHE.lock().unwrap().as_ref().filter(|(at, _)| !q.refresh && at.elapsed() < KEEP).map(|(at, v)| (*at, v.clone()));
    let sources = s.installer.sources();
    let (checked_at, remote) = match cached {
        Some(c) => c,
        None => {
            let api = s.installer.api();
            let core_key = s.installer.core_key();
            let wanted: Vec<(String, String)> = list
                .iter()
                .map(|e| {
                    let key = sources
                        .values()
                        .find(|src| src.repo.eq_ignore_ascii_case(&e.repo))
                        .map(|src| src.key.clone())
                        .filter(|k| !k.is_empty())
                        .unwrap_or_else(|| core_key.clone());
                    (e.repo.clone(), key)
                })
                .collect();
            let remote = blocking(move || {
                std::thread::scope(|sc| {
                    let handles: Vec<_> = wanted.iter().map(|(repo, key)| sc.spawn(|| (repo.clone(), look(&api, repo, key)))).collect();
                    handles.into_iter().filter_map(|h| h.join().ok()).collect::<Vec<_>>()
                })
            })
            .await?;
            let now = Instant::now();
            *CACHE.lock().unwrap() = Some((now, remote.clone()));
            (now, remote)
        }
    };
    let apps = s.apps.list();
    let out: Vec<Value> = list
        .iter()
        .map(|e| {
            let r = remote.iter().find(|(repo, _)| repo == &e.repo).map(|(_, r)| r.clone()).unwrap_or_default();
            // Installée : venue de ce dépôt, ou du même identifiant.
            let id = sources.iter().find(|(_, src)| src.repo.eq_ignore_ascii_case(&e.repo)).map(|(id, _)| id.clone()).or(r.app.clone());
            let installed = id.as_deref().and_then(|id| apps.iter().find(|a| a.manifest.id == id));
            json!({
                "repo": e.repo,
                "name": e.name,
                "icon": e.icon,
                "description": if r.description.is_empty() { e.summary.clone() } else { r.description.clone() },
                "app": id,
                "version": r.version,
                "error": r.error,
                "installed": installed.map(|a| a.manifest.version.to_string()),
                "active": installed.is_some_and(|a| a.active),
            })
        })
        .collect();
    Ok(Json(json!({ "apps": out, "checkedSecsAgo": checked_at.elapsed().as_secs() })))
}

pub fn routes() -> Router<Shared> {
    Router::new().route("/api/apps/catalog", get(index))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_catalog_reads() {
        let e = super::entries();
        assert!(e.len() >= 8);
        assert!(e.iter().all(|x| crate::install::parse_repo(&x.repo).is_ok() && !x.name.is_empty()));
    }
}
