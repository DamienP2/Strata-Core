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
    /// Les sorties SDI tenues par le Core (avec MXL).
    pub playouts: crate::hardware::playout::Playouts,
    /// Les destinations NDI.
    pub ndi_outputs: crate::hardware::ndi_out::NdiOutputs,
    /// Les générateurs de mires (Santé › Mires et analyse).
    pub testgen: crate::testgen::Engine,
    /// Les aperçus des réglages (entrées, sorties, flux), seulement pendant qu'on regarde.
    pub previews: crate::hardware::preview::Previews,
    /// Les flux ST 2110 reçus et envoyés par le Core (MTL).
    pub st2110: crate::hardware::st2110io::Engine,
    /// Les flux réseau (SRT, RTMP, HLS…) reçus et envoyés par le Core.
    pub streams: crate::streams::engine::Engine,
    /// NMOS (IS-04, IS-05) : l'API du nœud et son annonce.
    pub nmos: crate::nmos::Nmos,
    /// Les fonctions activées ou coupées (Réglages › Fonctions).
    pub features: crate::features::Features,
    /// SAP : flux de son annoncés (Dante, AES67).
    pub sap: crate::hardware::sap::Sap,
    /// Les relevés des ports média ST 2110 (débits).
    pub port_meter: crate::hardware::capacity::Meter,
    /// MTL : chargée au démarrage si elle est installée (sinon la raison).
    pub mtl: std::sync::OnceLock<Result<std::path::PathBuf, String>>,
    /// MXL : ouvert au démarrage s'il est installé (sinon la raison).
    pub mxl: std::sync::OnceLock<Result<Arc<strata_mxl::Mxl>, String>>,
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
    /// Le Core s'arrête : les entrées et sorties ne sont plus (re)lancées.
    pub stopping: AtomicBool,
    /// Une modification de `config.json` à la fois (voir `update_config`).
    config_write: Mutex<()>,
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
            playouts: Default::default(),
            ndi_outputs: Default::default(),
            testgen: Default::default(),
            previews: Default::default(),
            mxl: Default::default(),
            st2110: Default::default(),
            streams: Default::default(),
            nmos: Default::default(),
            features: crate::features::Features::load(&data_dir),
            sap: Default::default(),
            port_meter: Default::default(),
            mtl: Default::default(),
            resources: Default::default(),
            alerts: crate::alerts::Alerts::new(&data_dir),
            snmp_settings: Mutex::new(crate::snmp::SnmpSettings::load(&data_dir)),
            snmp: Default::default(),
            cluster: crate::cluster::Cluster::load(&data_dir),
            restart: AtomicBool::new(false),
            restart_signal: tokio::sync::Notify::new(),
            started_at: Instant::now(),
            stopping: AtomicBool::new(false),
            config_write: Mutex::new(()),
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

    /// Change les réglages du serveur : relus, changés par `f`, enregistrés puis pris, sous un
    /// verrou (deux modifications en même temps, le nom et le rattachement au cockpit par exemple,
    /// ne s'effacent pas l'une l'autre). Bloquant (écrit le fichier).
    pub fn update_config<R>(&self, f: impl FnOnce(&mut ServerConfig) -> Result<R, String>) -> Result<R, String> {
        let _one = self.config_write.lock().unwrap_or_else(|e| e.into_inner());
        let mut cfg = (**self.config.load()).clone();
        let r = f(&mut cfg)?;
        cfg.save(&self.data_dir).map_err(|e| format!("cannot save the settings: {e}"))?;
        self.config.store(Arc::new(cfg));
        Ok(r)
    }

    pub fn restart_asked(&self) -> bool {
        self.restart.load(Ordering::SeqCst)
    }
}
