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
pub mod backup;
pub mod cli;
pub mod cluster;
pub mod config;
pub mod groups;
pub mod guard;
pub mod hardware;
pub mod health;
pub mod install;
pub mod logs;
pub mod nmos;
pub mod prefs;
pub mod recette;
pub mod relay;
pub mod resources;
pub mod server;
pub mod snmp;
pub mod sso;
pub mod state;
pub mod streams;
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
        .layer(axum::middleware::from_fn_with_state(s.clone(), auth::refresh_cookie))
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
    // Une restauration préparée avant le redémarrage : en place avant de rien lire.
    backup::apply_pending(&data_dir);
    boot_guard(&data_dir);
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

/// Le Core a planté plusieurs fois de suite au démarrage : il démarre sans MXL ni MTL
/// (bibliothèques natives), pour qu'on garde la main.
static SAFE_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn safe_mode() -> bool {
    SAFE_MODE.load(std::sync::atomic::Ordering::Relaxed)
}

/// Les démarrages ratés de suite (le Core mort en moins d'une minute) : au 4e, mode sans échec.
/// Le compte repart à zéro après une minute de marche.
fn boot_guard(data_dir: &std::path::Path) {
    let path = data_dir.join("health").join("boot.json");
    let quick = std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0) + 1;
    let _ = std::fs::create_dir_all(data_dir.join("health"));
    let _ = std::fs::write(&path, quick.to_string());
    if quick > 3 {
        SAFE_MODE.store(true, std::sync::atomic::Ordering::Relaxed);
        tracing::error!("the Core stopped {} times in a row at start: safe mode, MXL and MTL are not loaded", quick - 1);
    }
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        let _ = std::fs::write(&path, "0");
    });
}

/// Essaie une bibliothèque native dans un processus à part (`strata-core lib-probe <mxl|mtl>`) :
/// une bibliothèque qui plante (instruction que ce processeur n'a pas…) ne fait pas tomber le Core.
fn probe(lib: &str) -> Result<(), String> {
    use std::os::unix::process::ExitStatusExt;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // Seulement le vrai programme (un binaire de test ne connaît pas `lib-probe`).
    if !exe.file_name().is_some_and(|n| n.to_string_lossy().starts_with("strata-core")) {
        return Ok(());
    }
    let mut child = std::process::Command::new(exe)
        .args(["lib-probe", lib])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => return Ok(()),
            // Pas trouvée, ou refusée : la vraie erreur vient au chargement.
            Ok(Some(st)) if st.code().is_some() => return Ok(()),
            Ok(Some(st)) => {
                return Err(format!(
                    "{lib} crashes on this server (signal {}): run the installer again and choose Update, or send the journal",
                    st.signal().unwrap_or(0)
                ))
            }
            Ok(None) if start.elapsed() > std::time::Duration::from_secs(30) => {
                let _ = child.kill();
                return Err(format!("{lib} does not load (no answer in 30 s)"));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// `strata-core lib-probe` : charge la bibliothèque et rend la main.
pub fn lib_probe(lib: &str) -> anyhow::Result<()> {
    match lib {
        "mxl" => strata_mxl::Mxl::open().map(|_| ()).map_err(|e| anyhow::anyhow!(e)),
        "mtl" => strata_mtl::load().map(|_| ()).map_err(|e| anyhow::anyhow!(e)),
        _ => Err(anyhow::anyhow!("mxl or mtl")),
    }
}

/// MXL : les entrées deviennent des flux MXL que les applis lisent (sinon, le partage
/// `strata-share`).
async fn open_mxl(s: &Shared) {
    if s.mxl.get().is_some() {
        return;
    }
    if safe_mode() {
        let why = "safe mode: the Core stopped several times at start".to_string();
        let _ = s.mtl.set(Err(why.clone()));
        let _ = s.mxl.set(Err(why));
        return;
    }
    let r = match tokio::task::spawn_blocking(|| probe("mxl")).await.unwrap_or_else(|e| Err(e.to_string())) {
        Ok(()) => tokio::task::spawn_blocking(strata_mxl::Mxl::open).await.unwrap_or_else(|e| Err(e.to_string())),
        Err(e) => Err(e),
    };
    match &r {
        Ok(m) => tracing::info!("MXL {} on {} ({})", m.version, m.domain().display(), m.library.display()),
        Err(e) => tracing::info!("no MXL ({e}): the inputs are shared with strata-share"),
    }
    let r = r.map(std::sync::Arc::new);
    // MTL (ST 2110 sur les ports média), qui passe par MXL.
    let mtl = match tokio::task::spawn_blocking(|| probe("mtl")).await.unwrap_or_else(|e| Err(e.to_string())) {
        Ok(()) => tokio::task::spawn_blocking(strata_mtl::load).await.unwrap_or_else(|e| Err(e.to_string())),
        Err(e) => Err(e),
    };
    match &mtl {
        Ok(p) => tracing::info!("MTL loaded ({})", p.display()),
        Err(e) => tracing::info!("no MTL ({e}): the ST 2110 flows are only set, not received or sent by the Core"),
    }
    let _ = s.mtl.set(mtl);
    s.captures.set_mxl(r.as_ref().ok().cloned());
    s.playouts.set_mxl(r.as_ref().ok().cloned());
    let _ = s.mxl.set(r);
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
    open_mxl(&s).await;
    reconcile_inputs(&s).await;
    resources::start(&s);
    alerts::start(&s);
    snmp::apply(&s).await;
    nmos::apply(&s).await;
    cluster::start(&s);
    backup::start(&s);
    let keeper = {
        let s = s.clone();
        tokio::spawn(async move {
            let mut tls_at = std::time::Instant::now();
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                apps::reconcile(&s).await;
                reconcile_inputs(&s).await;
                nmos::apply(&s).await;
                let s3 = s.clone();
                let _ = tokio::task::spawn_blocking(move || hardware::sap::apply(&s3)).await;
                let s2 = s.clone();
                let _ = tokio::task::spawn_blocking(move || s2.previews.sweep()).await;
                // La recherche quotidienne à part : sans internet, ses délais ne figent pas ce
                // gardien.
                let s2 = s.clone();
                tokio::spawn(async move { updates::tick(&s2).await });
                // Le certificat fait par Strata : revu toutes les 12 h (à l'horloge, pas au compte
                // des tours).
                if tls_at.elapsed() >= std::time::Duration::from_secs(12 * 3600) {
                    tls_at = std::time::Instant::now();
                    tls::apply(&s).await;
                }
            }
        })
    };
    axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).with_graceful_shutdown(stop(s.clone())).await?;
    keeper.abort();
    s.supervisor.stop_all().await;
    let s2 = s.clone();
    let _ = tokio::task::spawn_blocking(move || nmos::stop(&s2)).await;
    let s2 = s.clone();
    let _ = tokio::task::spawn_blocking(move || hardware::sap::stop(&s2)).await;
    // Les entrées après les applis qui les lisent.
    let s2 = s.clone();
    let _ = tokio::task::spawn_blocking(move || {
        hardware::stop_reconciling(&s2);
        s2.streams.stop_all();
        s2.st2110.stop_all();
        s2.playouts.stop_all();
        s2.captures.stop_all();
    })
    .await;
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
