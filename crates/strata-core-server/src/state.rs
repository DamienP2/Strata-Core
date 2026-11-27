//! Ce que chaque requête peut atteindre.
use arc_swap::ArcSwap;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU16, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};

use crate::{apps::Apps, auth::Auth, config::ServerConfig, groups::Groups, install::Installer, relay, supervisor::Supervisor, tls::Tls, updates::Updates};

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
    /// L'état de NTP et PTP (Matériel › Horloge).
    pub clock: crate::hardware::sync::Watch,
    /// Les entrées SDI captées et partagées.
    pub captures: crate::hardware::capture::Captures,
    /// Les aperçus des réglages (entrées, sorties, flux), seulement pendant qu'on regarde.
    pub previews: crate::hardware::preview::Previews,
    /// Les relevés des ressources (Santé).
    pub resources: crate::resources::Resources,
    /// Les alertes (Santé).
    pub alerts: crate::alerts::Alerts,
    /// Le SNMP : ses réglages, l'agent.
    pub snmp_settings: Mutex<crate::snmp::SnmpSettings>,
    pub snmp: crate::snmp::Agent,
    /// Les autres serveurs (maître et membres).
    pub cluster: crate::cluster::Cluster,
    /// Les mises à jour du Core et des applis.
    pub updates: Updates,
    /// Le HTTPS : certificat et écoute.
    pub tls: Tls,
    /// Un redémarrage est demandé.
    restart: AtomicBool,
    pub restart_signal: tokio::sync::Notify,
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
            tls: Tls::new(&data_dir),
            updates: Updates::default(),
            clock: Default::default(),
            captures: Default::default(),
            previews: Default::default(),
            resources: Default::default(),
            alerts: crate::alerts::Alerts::new(&data_dir),
            snmp_settings: Mutex::new(crate::snmp::SnmpSettings::load(&data_dir)),
            snmp: Default::default(),
            cluster: crate::cluster::Cluster::load(&data_dir),
            restart: AtomicBool::new(false),
            restart_signal: tokio::sync::Notify::new(),
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

    /// Demande au service de s'arrêter pour redémarrer.
    pub fn ask_restart(&self) {
        self.restart.store(true, Ordering::SeqCst);
        self.restart_signal.notify_waiters();
    }

    pub fn restart_asked(&self) -> bool {
        self.restart.load(Ordering::SeqCst)
    }
}
