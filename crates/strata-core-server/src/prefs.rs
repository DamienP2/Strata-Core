//! Les choix de chaque utilisateur gardés sur le serveur : langue et thème, les mêmes dans toutes
//! les applis et sur tous les navigateurs.
use axum::{extract::State, routing::put, Json, Router};
use serde::Deserialize;

use crate::{
    api::{bad, blocking, ApiResult},
    auth::{CurrentUser, PublicUser},
    state::Shared,
};

pub fn routes() -> Router<Shared> {
    Router::new().route("/api/me/prefs", put(set))
}

#[derive(Deserialize)]
struct Body {
    lang: Option<String>,
    theme: Option<String>,
}

async fn set(State(s): State<Shared>, CurrentUser(me): CurrentUser, Json(b): Json<Body>) -> ApiResult<Json<PublicUser>> {
    if let Some(l) = &b.lang {
        if !matches!(l.as_str(), "" | "en" | "fr") {
            return Err(bad("unknown language"));
        }
    }
    if let Some(t) = &b.theme {
        if !matches!(t.as_str(), "" | "dark" | "light" | "system") {
            return Err(bad("unknown theme"));
        }
    }
    let auth = s.auth.clone();
    let user = blocking(move || {
        auth.modify(&me.id, |u| {
            if let Some(l) = b.lang {
                u.lang = l;
            }
            if let Some(t) = b.theme {
                u.theme = t;
            }
            Ok(())
        })
    })
    .await?
    .map_err(bad)?;
    Ok(Json(user.public()))
}
