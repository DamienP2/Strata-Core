//! Ce que chaque requête peut atteindre.
use arc_swap::ArcSwap;
use std::{path::PathBuf, sync::Arc, time::Instant};

use crate::{apps::Apps, auth::Auth, config::ServerConfig, groups::Groups};

pub type Shared = Arc<AppState>;

pub struct AppState {
    pub data_dir: PathBuf,
    pub auth: Arc<Auth>,
    pub config: ArcSwap<ServerConfig>,
    pub apps: Apps,
    pub groups: Groups,
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
            started_at: Instant::now(),
            data_dir,
        })
    }
}
