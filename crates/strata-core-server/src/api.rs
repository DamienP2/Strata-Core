//! L'API HTTP : aides communes et routeur. Repris de LiveCut : erreurs en texte court, anglais,
//! minuscules (« wrong user name or password ») ; l'interface les traduit.
use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde_json::{json, Value};

use crate::state::Shared;

pub type ApiResult<T> = Result<T, (StatusCode, String)>;

pub fn bad(e: impl ToString) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, e.to_string())
}

pub fn internal(e: impl ToString) -> (StatusCode, String) {
    let e = e.to_string();
    tracing::error!("{e}");
    (StatusCode::INTERNAL_SERVER_ERROR, e)
}

pub fn require(cond: bool) -> ApiResult<()> {
    if cond {
        Ok(())
    } else {
        Err((StatusCode::FORBIDDEN, "you do not have the rights for this".into()))
    }
}

/// Le travail bloquant (Argon2, fichiers) hors des fils asynchrones.
pub async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> ApiResult<T> {
    tokio::task::spawn_blocking(f).await.map_err(internal)
}

pub fn router() -> Router<Shared> {
    Router::new()
        .route("/api/health", get(health))
        .merge(crate::auth::routes())
        .merge(crate::prefs::routes())
        .merge(crate::apps::routes())
        .merge(crate::users::routes())
        .merge(crate::groups::routes())
}

/// Supervision, sans connexion : le serveur répond-il, en quelle version, attend-il son premier compte ?
async fn health(State(s): State<Shared>) -> Json<Value> {
    let cfg = s.config.load();
    Json(json!({
        "name": cfg.name,
        "version": strata_core::VERSION,
        "needsSetup": s.auth.needs_setup(),
        "uptimeSecs": s.started_at.elapsed().as_secs(),
    }))
}
