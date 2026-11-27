//! Les groupes du Core (Réglages › Personnes › Groupes) : un nom, et les groupes de l'annuaire qui le
//! remplissent. Les applis donnent leurs droits à ces groupes.
//!
//! Un compte local est mis dans ses groupes à la main ; un compte de l'annuaire (SSO) reçoit, à chaque
//! connexion, les groupes du Core associés à ses groupes de l'annuaire.
use axum::{
    extract::{Path as UrlPath, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use crate::{
    api::{bad, require, ApiResult},
    auth::CurrentUser,
    state::Shared,
};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub id: String,
    pub name: String,
    /// Les groupes de l'annuaire qui le remplissent (sans casse, `/` de tête ignoré).
    #[serde(default)]
    pub directory: Vec<String>,
}

pub struct Groups {
    path: PathBuf,
    list: Mutex<Vec<Group>>,
}

/// Un nom de groupe de l'annuaire, comparable : minuscules, sans `/` de tête (Keycloak).
pub fn clean_directory_group(g: &str) -> String {
    g.trim().trim_start_matches('/').to_lowercase()
}

fn clean_list(v: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for g in v {
        let g = g.trim().to_string();
        if !g.is_empty() && !out.iter().any(|o| clean_directory_group(o) == clean_directory_group(&g)) {
            out.push(g);
        }
    }
    out
}

impl Groups {
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("groups.json");
        let list = strata_core::files::read_json(&path).unwrap_or_default();
        Groups { path, list: Mutex::new(list) }
    }

    pub fn list(&self) -> Vec<Group> {
        self.list.lock().unwrap().clone()
    }

    pub fn exists(&self, id: &str) -> bool {
        self.list.lock().unwrap().iter().any(|g| g.id == id)
    }

    /// Les noms de ces groupes, dans l'ordre des noms.
    pub fn names_of(&self, ids: &[String]) -> Vec<String> {
        let list = self.list.lock().unwrap();
        let mut names: Vec<String> = list.iter().filter(|g| ids.contains(&g.id)).map(|g| g.name.clone()).collect();
        names.sort_by_key(|n| n.to_lowercase());
        names
    }

    /// Les groupes du Core associés à ces groupes de l'annuaire.
    pub fn from_directory(&self, directory_groups: &[String]) -> Vec<String> {
        let mine: Vec<String> = directory_groups.iter().map(|g| clean_directory_group(g)).collect();
        self.list.lock().unwrap().iter().filter(|g| g.directory.iter().any(|d| mine.contains(&clean_directory_group(d)))).map(|g| g.id.clone()).collect()
    }

    /// Change la liste d'un coup : noms vérifiés (non vides, uniques sans casse), enregistrée.
    fn edit<R>(&self, f: impl FnOnce(&mut Vec<Group>) -> Result<R, String>) -> Result<R, String> {
        let mut list = self.list.lock().unwrap();
        let mut next = list.clone();
        let r = f(&mut next)?;
        let mut names = std::collections::HashSet::new();
        for g in &next {
            if g.name.trim().is_empty() {
                return Err("the group needs a name".into());
            }
            if g.name.chars().count() > 64 {
                return Err("the group name is too long (64 characters at most)".into());
            }
            if !names.insert(g.name.to_lowercase()) {
                return Err("this group name is already taken".into());
            }
        }
        strata_core::files::write_json(&self.path, &next, false).map_err(|e| format!("cannot save the groups: {e}"))?;
        *list = next;
        Ok(r)
    }

    pub fn add(&self, name: &str, directory: Vec<String>) -> Result<Group, String> {
        let g = Group { id: strata_core::new_id(), name: name.trim().to_string(), directory: clean_list(directory) };
        let out = g.clone();
        self.edit(|l| {
            l.push(g);
            Ok(())
        })?;
        Ok(out)
    }

    pub fn modify(&self, id: &str, name: Option<String>, directory: Option<Vec<String>>) -> Result<Group, String> {
        self.edit(|l| {
            let g = l.iter_mut().find(|g| g.id == id).ok_or_else(|| "no such group".to_string())?;
            if let Some(n) = name {
                g.name = n.trim().to_string();
            }
            if let Some(d) = directory {
                g.directory = clean_list(d);
            }
            Ok(g.clone())
        })
    }

    /// Les groupes du maître, sur un membre : ils remplacent les siens (même identifiants). Vrai si
    /// quelque chose a changé.
    pub fn mirror(&self, groups: Vec<(String, String)>) -> bool {
        let next: Vec<Group> = groups.into_iter().map(|(id, name)| Group { id, name, directory: Vec::new() }).collect();
        let mut list = self.list.lock().unwrap();
        if *list == next {
            return false;
        }
        if strata_core::files::write_json(&self.path, &next, false).is_err() {
            return false;
        }
        *list = next;
        true
    }

    pub fn delete(&self, id: &str) -> Result<Group, String> {
        self.edit(|l| {
            let i = l.iter().position(|g| g.id == id).ok_or_else(|| "no such group".to_string())?;
            Ok(l.remove(i))
        })
    }
}

pub fn routes() -> Router<Shared> {
    Router::new().route("/api/groups", get(list).post(create)).route("/api/groups/{id}", axum::routing::patch(update).delete(remove))
}

fn refused(e: String) -> (StatusCode, String) {
    if e == "no such group" {
        (StatusCode::NOT_FOUND, e)
    } else {
        bad(e)
    }
}

fn group_json(g: &Group, members: usize) -> Value {
    json!({"id": g.id, "name": g.name, "directory": g.directory, "members": members})
}

async fn list(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let users = s.auth.users();
    let mut groups = s.groups.list();
    groups.sort_by_key(|g| g.name.to_lowercase());
    Ok(Json(json!({
        "groups": groups.iter().map(|g| group_json(g, users.iter().filter(|u| u.groups.contains(&g.id)).count())).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
struct Body {
    name: Option<String>,
    directory: Option<Vec<String>>,
}

async fn create(State(s): State<Shared>, CurrentUser(me): CurrentUser, Json(b): Json<Body>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let g = s.groups.add(b.name.as_deref().unwrap_or(""), b.directory.unwrap_or_default()).map_err(bad)?;
    tracing::info!("group {} created by {}", g.name, me.username);
    Ok(Json(group_json(&g, 0)))
}

async fn update(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath(id): UrlPath<String>, Json(b): Json<Body>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let g = s.groups.modify(&id, b.name, b.directory).map_err(refused)?;
    let members = s.auth.users().iter().filter(|u| u.groups.contains(&g.id)).count();
    Ok(Json(group_json(&g, members)))
}

/// Supprime un groupe ; ses membres en sortent.
async fn remove(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath(id): UrlPath<String>) -> ApiResult<StatusCode> {
    require(me.admin)?;
    let g = s.groups.delete(&id).map_err(refused)?;
    let auth = s.auth.clone();
    crate::api::blocking(move || {
        auth.edit(|list| {
            for u in list.iter_mut() {
                u.groups.retain(|x| *x != g.id);
            }
            Ok(())
        })
    })
    .await?
    .map_err(bad)?;
    tracing::info!("group {id} deleted by {}", me.username);
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_and_directory() {
        let dir = std::env::temp_dir().join(format!("strata-groups-{}", strata_core::new_id()));
        let g = Groups::load(&dir);
        let real = g.add(" Réalisation ", vec!["/Realisateurs".into(), " ".into(), "REALISATEURS".into()]).unwrap();
        assert_eq!(real.name, "Réalisation");
        assert_eq!(real.directory, vec!["/Realisateurs".to_string()], "trimmed, empty and repeated entries dropped");
        let tech = g.add("Technique", vec!["it-video".into()]).unwrap();
        assert!(g.add("réalisation", vec![]).is_err(), "unique without case");
        assert!(g.add("  ", vec![]).is_err());
        assert_eq!(g.from_directory(&["realisateurs".into(), "other".into()]), vec![real.id.clone()]);
        assert_eq!(g.from_directory(&["/IT-Video".into(), "Realisateurs".into()]).len(), 2);
        assert_eq!(g.names_of(&[tech.id.clone(), real.id.clone()]), vec!["Réalisation", "Technique"]);
        g.modify(&tech.id, Some("Ingénierie".into()), Some(vec![])).unwrap();
        assert!(g.from_directory(&["it-video".into()]).is_empty());
        assert_eq!(Groups::load(&dir).list().len(), 2, "saved");
        g.delete(&tech.id).unwrap();
        assert!(g.delete(&tech.id).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
