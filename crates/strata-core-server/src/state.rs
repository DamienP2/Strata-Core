//! Ce que chaque requête peut atteindre.
use arc_swap::ArcSwap;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU16, Ordering},
        Arc,
    },
    time::Instant,
};

use crate::{apps::Apps, auth::Auth, config::ServerConfig, groups::Groups, install::Installer, relay, supervisor::Supervisor};

pub type Shared = Arc<AppState>;

pub struct AppState {
    pub data_dir: PathBuf,
    pub auth: Arc<Auth>,
    pub config: ArcSwap<ServerConfig>,
    pub apps: Apps,
    pub groups: Groups,
    /// Les applis lancées par le Core.
    pub supervisor: Supervisor,
    /// Les installations et mises à jour d'applis.
    pub installer: Installer,
    /// Le client du relais vers les applis.
    pub relay: relay::HttpClient,
    /// Le port où le Core écoute (donné aux applis : `STRATA_CORE_URL`).
    core_port: AtomicU16,
    pub started_at: Instant,
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Shared {
        let config = ServerConfig::load(&data_dir);
        Arc::new(AppState {
            auth: Arc::new(Auth::load(&data_dir)),
            config: ArcSwap::from_pointee(config),
            apps: Apps::new(&data_dir),
            groups: Groups::load(&data_dir),
            supervisor: Supervisor::default(),
            installer: Installer::new(&data_dir),
            relay: relay::client(),
            core_port: AtomicU16::new(8080),
            started_at: Instant::now(),
            data_dir,
        })
    }
}

impl AppState {
    pub fn core_port(&self) -> u16 {
        self.core_port.load(Ordering::Relaxed)
    }

    pub fn set_core_port(&self, port: u16) {
        self.core_port.store(port, Ordering::Relaxed);
    }
}
