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
        .merge(crate::install::routes())
        .merge(crate::server::routes())
        .merge(crate::updates::routes())
        .merge(crate::hardware::routes())
        .merge(crate::streams::api::routes())
        .merge(crate::health::routes())
        .merge(crate::resources::routes())
        .merge(crate::alerts::routes())
        .merge(crate::snmp::routes())
        .merge(crate::cluster::routes())
        .merge(crate::users::routes())
        .merge(crate::groups::routes())
        .merge(crate::sso::routes())
        .merge(crate::backup::routes())
        .merge(crate::storage::routes())
        .merge(crate::catalog::routes())
        .merge(crate::features::routes())
}

/// Supervision, sans connexion : le serveur répond-il, en quelle version, attend-il son premier compte ?
async fn health(State(s): State<Shared>) -> Json<Value> {
    let cfg = s.config.load();
    Json(json!({
        "name": cfg.name,
        "version": strata_core::VERSION,
        "needsSetup": s.auth.needs_setup(),
        "uptimeSecs": s.started_at.elapsed().as_secs(),
        // L'interface bloque l'écran pendant une mise à jour ou un redémarrage.
        "updating": s.updates.busy(),
        "restarting": s.restart_asked(),
        "safeMode": crate::safe_mode(),
        "unavailable": unavailable(&s),
    }))
}

/// Ce qui ne marche pas faute d'une bibliothèque (absente, ou qui plante) : le Core tourne sans,
/// et l'interface dit quelles fonctions manquent.
fn unavailable(s: &crate::state::AppState) -> Vec<Value> {
    let mut v = Vec::new();
    if let Some(Err(e)) = s.mxl.get() {
        v.push(json!({ "lib": "MXL", "features": "mxl", "why": e }));
    }
    // MTL n'est attendu que s'il plante ou si le Core est en mode sans échec (sans carte Intel,
    // son absence est normale : la page ST 2110 le dit).
    if let Some(Err(e)) = s.mtl.get() {
        if crate::safe_mode() || e.contains("crashes") {
            v.push(json!({ "lib": "MTL", "features": "mtl", "why": e }));
        }
    }
    if let Err(e) = s.streams.ffmpeg() {
        v.push(json!({ "lib": "ffmpeg", "features": "ffmpeg", "why": e }));
    }
    v
}
