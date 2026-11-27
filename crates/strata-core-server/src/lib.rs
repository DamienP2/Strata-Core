//! Le service `strata-core` : le socle d'un serveur Strata.
//!
//! Aujourd'hui : le cockpit (les applis actives de ce serveur), les comptes et les sessions, le SSO,
//! les groupes, les préférences des utilisateurs, la Bibliothèque, le lancement des applis et le
//! relais vers elles, l'interface web embarquée. À venir (brief) : installer une appli, matériel,
//! santé, mises à jour, plusieurs serveurs.
pub mod alerts;
pub mod api;
pub mod apps;
pub mod auth;
pub mod cli;
pub mod cluster;
pub mod config;
pub mod groups;
pub mod guard;
pub mod hardware;
pub mod health;
pub mod install;
pub mod logs;
pub mod prefs;
pub mod relay;
pub mod resources;
pub mod server;
pub mod snmp;
pub mod sso;
pub mod state;
pub mod supervisor;
pub mod tls;
pub mod updates;
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
        .layer(axum::middleware::from_fn_with_state(s.clone(), tls::redirect))
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

/// Comment le service s'est arrêté.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Ending {
    Stop,
    /// Un administrateur a demandé un redémarrage : le programme se relance.
    Restart,
}

/// Sert jusqu'à Ctrl-C, SIGTERM ou une demande de redémarrage.
pub async fn serve(bind: Option<SocketAddr>, data_dir: PathBuf, web_dir: Option<PathBuf>) -> anyhow::Result<Ending> {
    std::fs::create_dir_all(&data_dir)?;
    let s = state::AppState::new(data_dir.clone());
    let bind = bind.or(s.config.load().bind).unwrap_or(DEFAULT_BIND.parse()?);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!("Strata Core {} on http://{bind} (data: {})", strata_core::VERSION, data_dir.display());
    if s.auth.needs_setup() {
        tracing::warn!(
            "no user yet: open http://localhost:{}/ on this machine to create the administrator account, or run `strata-core user add admin`",
            bind.port()
        );
    }
    serve_on(s, listener, web_dir).await
}

/// Sert sur une écoute HTTP déjà ouverte (et en HTTPS si les réglages le demandent).
pub async fn serve_on(s: Shared, listener: tokio::net::TcpListener, web_dir: Option<PathBuf>) -> anyhow::Result<Ending> {
    let addr = listener.local_addr()?;
    s.set_core_port(addr.port());
    let router = app(s.clone(), web_dir);
    s.tls.set_router(router.clone(), addr);
    tls::apply(&s).await;
    // Les applis actives : lancées maintenant, puis tenues en accord avec la Bibliothèque. Le
    // certificat fait par Strata : renouvelé quand il le faut.
    apps::reconcile(&s).await;
    reconcile_inputs(&s).await;
    resources::start(&s);
    alerts::start(&s);
    snmp::apply(&s).await;
    cluster::start(&s);
    let keeper = {
        let s = s.clone();
        tokio::spawn(async move {
            let mut tick = 0u64;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                apps::reconcile(&s).await;
                reconcile_inputs(&s).await;
                let s2 = s.clone();
                let _ = tokio::task::spawn_blocking(move || s2.previews.sweep()).await;
                updates::tick(&s).await;
                tick += 1;
                if tick.is_multiple_of(12 * 3600 / 5) {
                    tls::apply(&s).await;
                }
            }
        })
    };
    axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).with_graceful_shutdown(stop(s.clone())).await?;
    keeper.abort();
    s.supervisor.stop_all().await;
    // Les entrées après les applis qui les lisent.
    let s2 = s.clone();
    let _ = tokio::task::spawn_blocking(move || s2.captures.stop_all()).await;
    // Un arrêt voulu : pas un plantage au prochain démarrage.
    health::stopped();
    s.auth.flush_sessions();
    let ending = if s.restart_asked() { Ending::Restart } else { Ending::Stop };
    tracing::info!("{}", if ending == Ending::Restart { "stopped, restarting" } else { "stopped" });
    Ok(ending)
}

/// Les entrées SDI captées, en accord avec les réglages et les cartes présentes.
async fn reconcile_inputs(s: &Shared) {
    let s = s.clone();
    let _ = tokio::task::spawn_blocking(move || hardware::reconcile_inputs(&s)).await;
}

async fn stop(s: Shared) {
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
        _ = s.restart_signal.notified() => {},
    }
}
