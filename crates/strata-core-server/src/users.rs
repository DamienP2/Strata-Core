//! Les comptes, pour les administrateurs (Réglages › Personnes › Utilisateurs).
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    api::{bad, blocking, require, ApiResult},
    auth::{CurrentUser, NewUser, PublicUser},
    state::Shared,
};

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/users", get(list).post(create))
        .route("/api/users/{id}", axum::routing::patch(update).delete(remove))
        .route("/api/users/{id}/password", post(password))
}

/// Une erreur de la liste des comptes : « no such user » en 404, le reste en 400.
fn refused(e: String) -> (StatusCode, String) {
    if e == "no such user" {
        (StatusCode::NOT_FOUND, e)
    } else {
        bad(e)
    }
}

async fn list(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let mut users: Vec<PublicUser> = s.auth.users().iter().map(|u| u.public()).collect();
    users.sort_by_key(|u| u.display_name.to_lowercase());
    Ok(Json(json!({ "users": users })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateBody {
    username: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    admin: bool,
    password: String,
}

async fn create(State(s): State<Shared>, CurrentUser(me): CurrentUser, Json(b): Json<CreateBody>) -> ApiResult<Json<PublicUser>> {
    require(me.admin)?;
    let auth = s.auth.clone();
    let user = blocking(move || auth.add(NewUser { username: b.username, display_name: b.display_name, admin: b.admin, password: Some(b.password) }))
        .await?
        .map_err(bad)?;
    tracing::info!("user {} created by {}", user.username, me.username);
    Ok(Json(user.public()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateBody {
    display_name: Option<String>,
    admin: Option<bool>,
    disabled: Option<bool>,
}

async fn update(State(s): State<Shared>, CurrentUser(me): CurrentUser, Path(id): Path<String>, Json(b): Json<UpdateBody>) -> ApiResult<Json<PublicUser>> {
    require(me.admin)?;
    if id == me.id && (b.disabled == Some(true) || b.admin == Some(false)) {
        return Err(bad("you cannot disable your own account or take away your own admin rights"));
    }
    let auth = s.auth.clone();
    let user = blocking(move || {
        auth.modify(&id, |u| {
            if let Some(n) = b.display_name {
                let n = n.trim();
                u.display_name = if n.is_empty() { u.username.clone() } else { n.to_string() };
            }
            if let Some(a) = b.admin {
                u.admin = a;
            }
            if let Some(d) = b.disabled {
                u.disabled = d;
            }
            Ok(())
        })
    })
    .await?
    .map_err(refused)?;
    Ok(Json(user.public()))
}

#[derive(Deserialize)]
struct PasswordBody {
    password: String,
}

/// Un nouveau mot de passe : les sessions du compte s'arrêtent.
async fn password(State(s): State<Shared>, CurrentUser(me): CurrentUser, Path(id): Path<String>, Json(b): Json<PasswordBody>) -> ApiResult<StatusCode> {
    require(me.admin)?;
    let auth = s.auth.clone();
    blocking(move || auth.set_password(&id, &b.password)).await?.map_err(refused)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove(State(s): State<Shared>, CurrentUser(me): CurrentUser, Path(id): Path<String>) -> ApiResult<StatusCode> {
    require(me.admin)?;
    if id == me.id {
        return Err(bad("you cannot delete your own account"));
    }
    let auth = s.auth.clone();
    let user = blocking(move || auth.delete(&id)).await?.map_err(refused)?;
    tracing::info!("user {} deleted by {}", user.username, me.username);
    Ok(StatusCode::NO_CONTENT)
}
