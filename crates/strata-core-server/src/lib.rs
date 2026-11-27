//! Le service `strata-core` : le socle d'un serveur Strata.
//!
//! Aujourd'hui : le cockpit (les applis actives de ce serveur), les comptes et les sessions, le SSO,
//! les groupes, les préférences des utilisateurs, la Bibliothèque, le lancement des applis et le
//! relais vers elles, l'interface web embarquée. À venir (brief) : installer une appli, matériel,
//! santé, mises à jour, plusieurs serveurs.
pub mod api;
pub mod apps;
pub mod auth;
pub mod cli;
pub mod config;
pub mod groups;
pub mod guard;
pub mod install;
pub mod prefs;
pub mod relay;
pub mod sso;
pub mod state;
pub mod supervisor;
pub mod users;
pub mod web;

use axum::Router;
use std::{net::SocketAddr, path::PathBuf};
use tower_http::services::ServeDir;

use state::Shared;

/// L'adresse d'écoute par défaut.
pub const DEFAULT_BIND: &str = "0.0.0.0:8080";

/// Le routeur complet : d'abord le relais (une requête pour `<appli>.<serveur>` part vers l'appli),
/// puis l'API et l'interface du Core (embarquée, ou `web_dir`).
pub fn app(s: Shared, web_dir: Option<PathBuf>) -> Router {
    let router = api::router();
    let router = match web_dir {
        Some(dir) => router.fallback_service(ServeDir::new(&dir).fallback(tower_http::services::ServeFile::new(dir.join("index.html")))),
        None => router.fallback(web::embedded),
    };
    router
        .layer(axum::middleware::from_fn_with_state(s.clone(), guard::csrf))
        .layer(axum::middleware::from_fn(guard::every_answer))
        .layer(axum::middleware::from_fn_with_state(s.clone(), relay::relay))
        .with_state(s)
}

/// Le dossier des données d'un lancement à la main (celui du service : /var/lib/strata-core).
pub fn user_data_dir() -> PathBuf {
    directories::ProjectDirs::from("com", "Strata", "Strata Core").map(|d| d.data_dir().to_path_buf()).unwrap_or_else(|| PathBuf::from("data"))
}

pub const SERVICE_DATA_DIR: &str = "/var/lib/strata-core";

/// Le dossier du service installé s'il existe, sinon celui de l'utilisateur (commandes `user`, `app`).
pub fn cli_data_dir(given: Option<PathBuf>) -> PathBuf {
    given.unwrap_or_else(|| {
        let svc = PathBuf::from(SERVICE_DATA_DIR);
        if svc.join("config.json").exists() {
            svc
        } else {
            user_data_dir()
        }
    })
}

/// Sert jusqu'à Ctrl-C ou SIGTERM.
pub async fn serve(bind: Option<SocketAddr>, data_dir: PathBuf, web_dir: Option<PathBuf>) -> anyhow::Result<()> {
    std::fs::create_dir_all(&data_dir)?;
    let s = state::AppState::new(data_dir.clone());
    let bind = bind.or(s.config.load().bind).unwrap_or(DEFAULT_BIND.parse()?);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    s.set_core_port(listener.local_addr()?.port());
    // Les applis actives : lancées maintenant, puis tenues en accord avec la Bibliothèque.
    apps::reconcile(&s).await;
    let keeper = {
        let s = s.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                apps::reconcile(&s).await;
            }
        })
    };
    tracing::info!("Strata Core {} on http://{bind} (data: {})", strata_core::VERSION, data_dir.display());
    if s.auth.needs_setup() {
        tracing::warn!(
            "no user yet: open http://localhost:{}/ on this machine to create the administrator account, or run `strata-core user add admin`",
            bind.port()
        );
    }
    axum::serve(listener, app(s.clone(), web_dir).into_make_service_with_connect_info::<SocketAddr>()).with_graceful_shutdown(stop()).await?;
    keeper.abort();
    s.supervisor.stop_all().await;
    tracing::info!("stopped");
    Ok(())
}

async fn stop() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
}
