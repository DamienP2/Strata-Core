//! Le matériel du serveur (Réglages › Matériel), réglé une fois pour toutes les applis : une entrée
//! SDI est une entrée pour toutes (brief § 5.3).
//!
//! - **Cartes SDI** : les cartes DeckLink que le pilote voit (`decklink.rs`), et pour chacune son
//!   nom, la fonction de chaque connecteur (entrée, sortie, inutilisé) et à quoi il est relié, et
//!   le genlock.
//! - **Réseau** : les cartes réseau (`net.rs`) et leurs rôles : gestion, NDI, les deux ports média
//!   du ST 2110 (en SMPTE 2022-7, rouge et bleu ; ou indépendants), PTP.
//! - **NDI** : la bibliothèque, les réglages que tous les programmes partagent (`ndi.rs`), les
//!   sources visibles.
//! - **ST 2110** : les flux (nom, sens, adresses multicast : rouge et bleue en 2022-7, une seule
//!   et son port sinon).
//! - **Horloge** : le domaine PTP, le décalage TAI, et l'état de NTP et PTP (`sync.rs`).
//! - **GPU** : NVIDIA, Intel, AMD, leur pilote et ce qu'ils encodent (`gpu.rs`).
//! - **Entrées** : chaque entrée SDI captée une fois par le Core et partagée avec les applis
//!   (`capture.rs`, `strata-share`).
//! - **Aperçus** : entrées et sorties SDI, flux ST 2110, une image par seconde seulement pendant
//!   qu'on regarde (`preview.rs`, `st2110.rs`).
//!
//! Gardé dans `hardware.json`. Les applis le lisent par `GET /api/hardware/shared` (docs/APPLIS.md).
pub mod capture;
pub mod clock;
pub mod decklink;
pub mod gpu;
pub mod media;
pub mod ndi;
pub mod net;
pub mod playout;
pub mod preview;
pub mod st2110;
pub mod st2110io;
pub mod sync;

use axum::{
    extract::{ConnectInfo, Path as UrlPath, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    path::{Path, PathBuf},
};

use crate::{
    api::{bad, blocking, require, ApiResult},
    auth::{CurrentUser, MaybeUser},
    guard,
    state::Shared,
};

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct HardwareConfig {
    /// Les cartes DeckLink réglées (par leur identité détectée).
    pub cards: Vec<Card>,
    pub network: NetworkRoles,
    pub ndi: NdiSettings,
    pub flows: Vec<Flow>,
    pub ptp: PtpSettings,
    /// L'heure du serveur, la cadence et le timecode maison.
    pub clock: clock::ClockSettings,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Card {
    /// L'identité de la carte détectée (`DetectedCard::id`), stable d'un démarrage à l'autre.
    pub id: String,
    /// Son nom d'usage (« Carte plateau »).
    pub name: String,
    /// Le modèle vu la dernière fois (« DeckLink Quad 2 »).
    pub model_name: String,
    /// Ses connecteurs SDI, dans l'ordre (SDI 1 d'abord).
    pub connectors: Vec<Connector>,
    /// Ses sorties suivent l'entrée de référence.
    pub genlock: bool,
    /// Décalage du genlock, en lignes.
    pub genlock_offset: i32,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Connector {
    /// À quoi il est relié (« Mélangeur clé 3 »).
    pub name: String,
    pub function: Function,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Function {
    #[default]
    Unused,
    Input,
    Output,
}

/// Quelle carte réseau fait quoi, par son nom (`eno1`, `ens2f0` : les noms restent quand les
/// adresses changent). Vide : aucune en particulier.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct NetworkRoles {
    /// Les postes de la régie joignent le cockpit par celle-ci.
    pub management: String,
    /// NDI envoie et cherche ses sources par celle-ci seulement.
    pub ndi: String,
    /// Les deux ports média du ST 2110 et leur usage.
    pub media_mode: MediaMode,
    /// Port média 1 (en 2022-7 : le réseau rouge, principal).
    pub media1: String,
    /// Port média 2 (en 2022-7 : le réseau bleu, de secours).
    pub media2: String,
    /// L'interface où tourne ptp4l (son horloge règle celle du système par phc2sys).
    pub ptp: String,
    /// Comment le Core reçoit et envoie les flux ST 2110 (MTL) : pas du tout (l'aperçu seulement),
    /// par le noyau, en AF_XDP, ou en DPDK (le port est alors retiré au noyau).
    pub media_io: MediaIo,
    /// En DPDK, l'adresse du port média 1 (« 10.10.1.21/24 ») : le noyau ne le voit plus.
    pub media1_address: String,
    pub media2_address: String,
}

/// Comment le Core tient les flux ST 2110.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MediaIo {
    /// Les flux ne sont que réglés (et regardés dans l'aperçu) : les applis les reçoivent elles-mêmes.
    #[default]
    Off,
    /// MTL par les sockets du noyau : simple, pour quelques flux HD.
    Kernel,
    /// MTL en AF_XDP : le port reste au noyau, bien plus rapide.
    AfXdp,
    /// MTL en DPDK : le plus rapide (UHD, nombreux flux) ; le port est réservé au ST 2110.
    Dpdk,
}

/// Comment servent les deux ports média.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MediaMode {
    /// SMPTE 2022-7 : chaque flux passe par les deux ports à la fois (rouge et bleu), l'un
    /// secourant l'autre.
    #[default]
    Redundant,
    /// Deux ports indépendants : chaque flux passe par l'un ou l'autre.
    Separate,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct NdiSettings {
    /// Le ou les serveurs de découverte NDI (`10.0.0.2`, séparés par des virgules) ; vide : mDNS.
    pub discovery: String,
    /// Le nom de machine des sources (`NOM (source)`) ; vide : celui de l'ordinateur.
    pub machine: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    #[default]
    Input,
    Output,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Essence {
    /// ST 2110-20.
    #[default]
    Video,
    /// ST 2110-30.
    Audio,
    /// ST 2110-40 (données annexes).
    Data,
}

/// Un flux ST 2110 : son adresse multicast (le groupe qu'un récepteur rejoint, ou où un émetteur
/// envoie). En 2022-7, une adresse sur chaque réseau (`red` par le port 1, `blue` par le port 2) ;
/// avec deux ports indépendants, une seule (`red`), par le port choisi (`port`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Flow {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub direction: Direction,
    #[serde(default)]
    pub essence: Essence,
    pub red: SocketAddrV4,
    #[serde(default)]
    pub blue: Option<SocketAddrV4>,
    /// Le port média (1 ou 2), avec deux ports indépendants.
    #[serde(default = "first_port")]
    pub port: u8,
    /// La vidéo (« 1080i50 », « 2160p50 ») ; vide : le format maison en 1080.
    #[serde(default)]
    pub format: String,
    /// Le type de charge RTP ; 0 : selon l'essence (96 vidéo, 97 son, 100 données).
    #[serde(default)]
    pub payload_type: u8,
    /// Son (ST 2110-30) : les canaux (2 par défaut).
    #[serde(default = "two")]
    pub channels: u16,
    /// Son : 24 (L24, par défaut) ou 16 bits par échantillon.
    #[serde(default = "l24")]
    pub bits: u8,
    /// Son : la durée d'un paquet en µs (1000 par défaut, AES67 ; 125 pour le niveau B/C).
    #[serde(default = "one_ms")]
    pub ptime_us: u32,
    /// Reçu ou envoyé par le Core ; coupé : réglé seulement (aussi par NMOS, `master_enable`).
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

fn two() -> u16 {
    2
}

fn l24() -> u8 {
    24
}

fn one_ms() -> u32 {
    1000
}

impl Flow {
    /// Le type de charge RTP envoyé. 96 sur un flux de son ou de données vient de l'ancien
    /// défaut (vidéo) : pris comme « selon l'essence ».
    pub fn pt(&self) -> u8 {
        match (self.essence, self.payload_type) {
            (Essence::Video, 0) => 96,
            (Essence::Audio, 0 | 96) => 97,
            (Essence::Data, 0 | 96) => 100,
            (_, p) => p,
        }
    }
}

/// Une vidéo d'après son nom : « 1080i50 », « 720p59.94 », « 2160p25 ».
pub fn parse_format(s: &str) -> Option<(u32, u32, (u32, u32), bool)> {
    let s = s.trim().to_ascii_lowercase();
    let (h, rest, interlaced) = if let Some((h, r)) = s.split_once('i') {
        (h, r, true)
    } else {
        let (h, r) = s.split_once('p')?;
        (h, r, false)
    };
    let height: u32 = h.parse().ok()?;
    let width = match height {
        480 => 720,
        576 => 720,
        720 => 1280,
        1080 => 1920,
        2160 => 3840,
        4320 => 7680,
        _ => return None,
    };
    // En entrelacé, le nom donne les trames par seconde (1080i50 : 25 images).
    let field_rate: f64 = rest.parse().ok()?;
    let frame_rate = if interlaced { field_rate / 2.0 } else { field_rate };
    let rate = if (frame_rate - frame_rate.round()).abs() < 0.001 { (frame_rate.round() as u32, 1) } else { ((frame_rate.ceil() as u32) * 1000, 1001) };
    Some((width, height, rate, interlaced))
}

fn first_port() -> u8 {
    1
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct PtpSettings {
    /// Domaine PTP (SMPTE ST 2059-2 : 127 par défaut).
    pub domain: u8,
    /// TAI − UTC en secondes (37 depuis 2017) : l'horloge du système est en UTC, PTP et RTP en TAI.
    pub tai_offset: u32,
}

impl Default for PtpSettings {
    fn default() -> Self {
        PtpSettings { domain: 127, tai_offset: 37 }
    }
}

fn is_multicast(a: &SocketAddrV4) -> bool {
    a.ip().is_multicast() && a.port() > 0
}

impl HardwareConfig {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("hardware.json")
    }

    pub fn load(data_dir: &Path) -> Self {
        strata_core::files::read_json(&Self::path(data_dir)).unwrap_or_default()
    }

    /// Des identifiants à tous les flux (`flow-1`, `flow-2`…), uniques.
    fn ensure_ids(&mut self) {
        let mut taken = HashSet::new();
        for f in &mut self.flows {
            if f.id.trim().is_empty() || !taken.insert(f.id.clone()) {
                let mut n = 1;
                while taken.contains(&format!("flow-{n}")) {
                    n += 1;
                }
                f.id = format!("flow-{n}");
                taken.insert(f.id.clone());
            }
        }
    }

    /// Nettoie et vérifie ; `detected` : les cartes vues, pour ce que chaque connecteur peut faire.
    pub fn check(&mut self, detected: &[decklink::DetectedCard]) -> Result<(), String> {
        self.ensure_ids();
        let mut ids = HashSet::new();
        for c in &mut self.cards {
            c.name = c.name.trim().to_string();
            if c.id.trim().is_empty() || !ids.insert(c.id.clone()) {
                return Err("each card is set once".into());
            }
            if c.genlock_offset.abs() > 2000 {
                return Err(format!("{}: the genlock offset is out of range", c.name));
            }
            if let Some(d) = detected.iter().find(|d| d.id == c.id) {
                if c.connectors.len() > d.connectors as usize {
                    return Err(format!("{} has {} connectors", d.model_name, d.connectors));
                }
                for (i, k) in c.connectors.iter().enumerate() {
                    let port = d.ports.get(i).copied().unwrap_or(decklink::Port::Both);
                    let ok = match k.function {
                        Function::Input => port != decklink::Port::Out,
                        Function::Output => port != decklink::Port::In,
                        Function::Unused => true,
                    };
                    if !ok {
                        return Err(format!(
                            "SDI {} of {} cannot be an {}",
                            i + 1,
                            d.model_name,
                            if k.function == Function::Input { "input" } else { "output" }
                        ));
                    }
                }
            }
            for k in &mut c.connectors {
                k.name = k.name.trim().to_string();
            }
        }
        let r = &self.network;
        if !r.media1.is_empty() && r.media1 == r.media2 {
            return Err("the two media ports need two different interfaces".into());
        }
        let redundant = r.media_mode == MediaMode::Redundant;
        let discovery = self.ndi.discovery.trim().to_string();
        if !discovery.chars().all(|c| c.is_ascii_alphanumeric() || ".-:, ".contains(c)) {
            return Err("the NDI discovery servers are addresses separated by commas".into());
        }
        self.ndi.discovery = discovery;
        self.ndi.machine = self.ndi.machine.trim().to_string();
        let mut used = HashSet::new();
        for f in &mut self.flows {
            f.name = f.name.trim().to_string();
            if f.name.is_empty() {
                return Err("each flow needs a name".into());
            }
            if redundant {
                f.port = 1;
                if f.blue.is_none() {
                    return Err(format!("{}: in SMPTE 2022-7, a flow has an address on each network (red and blue)", f.name));
                }
            } else {
                // Deux ports indépendants : une adresse, par le port choisi.
                f.blue = None;
                if !(1..=2).contains(&f.port) {
                    return Err(format!("{}: the media port is 1 or 2", f.name));
                }
            }
            for (leg, a) in [("red", Some(f.red)), ("blue", f.blue)] {
                let Some(a) = a else { continue };
                if !is_multicast(&a) {
                    return Err(format!("{}: the {leg} address must be multicast (224.0.0.0 to 239.255.255.255) with a port", f.name));
                }
                if !used.insert(a) {
                    return Err(format!("{a} is used by two flows"));
                }
            }
        }
        if self.ptp.domain > 127 {
            return Err("the PTP domain goes from 0 to 127".into());
        }
        if self.ptp.tai_offset > 100 {
            return Err("the TAI offset is in seconds (37 since 2017)".into());
        }
        self.clock.check()?;
        Ok(())
    }
}

/// Les endroits où Desktop Video installe sa bibliothèque.
const DESKTOP_VIDEO: &[&str] = &[
    "/usr/lib/libDeckLinkAPI.so",
    "/usr/lib64/libDeckLinkAPI.so",
    "/usr/lib/x86_64-linux-gnu/libDeckLinkAPI.so",
    "/usr/lib/aarch64-linux-gnu/libDeckLinkAPI.so",
];

/// Ce que voit la machine, et les réglages.
/// Un connecteur réglé en entrée que la carte ne sait plus capter (passée en full duplex : la
/// sortie d'une paire) redevient « inutilisé ». Vrai si quelque chose a changé.
fn fit_to_cards(cfg: &mut HardwareConfig, cards: &[decklink::DetectedCard]) -> bool {
    let mut changed = false;
    for c in &mut cfg.cards {
        let Some(d) = cards.iter().find(|d| d.id == c.id) else { continue };
        for (i, k) in c.connectors.iter_mut().enumerate() {
            let port = d.ports.get(i).copied().unwrap_or(decklink::Port::Both);
            if (k.function == Function::Input && port == decklink::Port::Out) || (k.function == Function::Output && port == decklink::Port::In) {
                k.function = Function::Unused;
                changed = true;
            }
        }
    }
    changed
}

fn snapshot(s: &Shared) -> Value {
    let mut cfg = HardwareConfig::load(&s.data_dir);
    let (cards, version, fake) = decklink::detect();
    fit_to_cards(&mut cfg, &cards);
    let path = DESKTOP_VIDEO.iter().find(|p| Path::new(p).exists());
    let ndi = match ndi::runtime() {
        Ok((path, version)) => json!({"path": path, "version": version}),
        Err(e) => json!({"error": e}),
    };
    json!({
        "config": cfg,
        "decklink": {
            "built": strata_decklink::BUILT,
            "driver": path.is_some() || fake,
            "driverPath": path,
            "version": version,
            "fake": fake,
            "cards": cards,
        },
        "models": decklink::MODELS,
        "nics": net::nics(),
        "gpus": gpu::detect(),
        "ndi": ndi,
    })
}

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/hardware", get(read).put(write))
        .route("/api/hardware/clock", get(clock))
        .route("/api/hardware/clock/apply", axum::routing::post(clock_apply))
        .route("/api/hardware/ndi/sources", get(ndi_sources))
        .route("/api/hardware/shared", get(shared))
        .route("/api/hardware/inputs", get(inputs))
        .route("/api/hardware/inputs/{card}/{n}/preview.jpg", get(preview))
        .route("/api/hardware/decklink/profile", axum::routing::post(decklink_profile))
        .route("/api/hardware/outputs", get(outputs))
        .route("/api/hardware/outputs/{card}/{n}/preview.jpg", get(output_preview))
        .route("/api/hardware/flows", get(flows))
        .route("/api/hardware/flows/{id}/preview.jpg", get(flow_preview))
}

/// Tient les captures en accord avec les réglages et les cartes présentes (bloquant).
pub fn reconcile_inputs(s: &crate::state::AppState) {
    let cfg = HardwareConfig::load(&s.data_dir);
    let (detected, _, simulated) = decklink::detect();
    let tz = s.config.load().timezone.clone();
    s.captures.reconcile(capture::wanted(&cfg, &detected, simulated, &tz));
    s.playouts.reconcile(playout::wanted(&cfg, &detected, simulated));
    let mtl = match s.mtl.get() {
        Some(Ok(_)) => Ok(()),
        Some(Err(e)) => Err(e.clone()),
        None => Err("not loaded yet".to_string()),
    };
    s.st2110.apply(&cfg, mtl, mxl_of(s));
    crate::streams::reconcile(s);
}

/// Les entrées captées : signal, format, images par seconde, applis qui les lisent.
async fn inputs(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let mut list = s.captures.list();
    for r in list.iter_mut().flat_map(|i| i.readers.iter_mut()) {
        r.name = s.apps.get(&r.app).map(|a| a.manifest.name);
    }
    Ok(Json(json!({ "inputs": list })))
}

/// MXL, s'il a été ouvert au démarrage.
pub fn mxl_of(s: &crate::state::AppState) -> Option<std::sync::Arc<strata_mxl::Mxl>> {
    s.mxl.get().and_then(|r| r.as_ref().ok().cloned())
}

/// Le flux MXL d'une sortie SDI ou d'un flux ST 2110 : écrit par l'appli qui envoie.
pub fn out_flow(name: &str) -> strata_mxl::Uuid {
    strata_mxl::flow_id(name)
}

/// Ce qu'une appli écrit dans un flux MXL de sortie : actif ou non, son format.
fn published_mxl(mxl: &strata_mxl::Mxl, id: strata_mxl::Uuid) -> Value {
    if !mxl.is_active(id) {
        return json!({"sending": false, "mxlFlow": id.to_string()});
    }
    let format = mxl.video_reader(id).ok().map(|r| {
        let v = r.video;
        let rate = v.rate.0 as f64 / v.rate.1.max(1) as f64 * if v.interlaced { 2.0 } else { 1.0 };
        let rate = (rate * 100.0).round() / 100.0;
        format!("{}{}{}", v.height, if v.interlaced { 'i' } else { 'p' }, if rate.fract() == 0.0 { format!("{rate:.0}") } else { format!("{rate}") })
    });
    json!({"sending": true, "mxlFlow": id.to_string(), "format": format, "appName": mxl.flow_label(id)})
}

fn jpeg_response(jpeg: std::sync::Arc<Vec<u8>>) -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "no-store")], (*jpeg).clone())
}

/// L'image la plus récente d'une entrée, en petit (JPEG), une par seconde pour tous.
async fn preview(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath((card, n)): UrlPath<(String, u32)>) -> ApiResult<impl IntoResponse> {
    require(me.admin)?;
    let feed = match (s.captures.mxl_flow_of(&card, n).and_then(|f| f.parse().ok()), mxl_of(&s)) {
        (Some(id), Some(mxl)) => Some(preview::Feed::Mxl { mxl, id, reader: None }),
        _ => s.captures.share_of(&card, n).map(preview::Feed::share),
    };
    let feed = feed.ok_or((StatusCode::NOT_FOUND, "this input is not captured".to_string()))?;
    let s2 = s.clone();
    let key = format!("in/{card}/{n}");
    let jpeg = blocking(move || s2.previews.jpeg(&key, || feed)).await?.map_err(|e| (StatusCode::NOT_FOUND, e))?;
    Ok(jpeg_response(jpeg))
}

#[derive(Deserialize)]
struct ProfileBody {
    card: String,
    profile: String,
}

/// Change le mode des connecteurs d'une carte (half duplex : chacun à part ; full duplex : par
/// paires, une entrée et sa sortie). Les captures de la carte s'arrêtent le temps du changement.
async fn decklink_profile(State(s): State<Shared>, CurrentUser(me): CurrentUser, Json(b): Json<ProfileBody>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let s2 = s.clone();
    let what = format!("{} switched to {}", b.card, b.profile);
    let v = blocking(move || {
        let (cards, _, simulated) = decklink::detect();
        let card = cards.iter().find(|c| c.id == b.card).ok_or("no such card")?;
        s2.captures.stop_all();
        let r = decklink::set_card_profile(card, &b.profile, simulated);
        let (cards, _, _) = decklink::detect();
        let mut cfg = HardwareConfig::load(&s2.data_dir);
        if fit_to_cards(&mut cfg, &cards) {
            strata_core::files::write_json(&HardwareConfig::path(&s2.data_dir), &cfg, false).map_err(|e| format!("cannot save the hardware settings: {e}"))?;
        }
        reconcile_inputs(&s2);
        r?;
        Ok::<_, String>(snapshot(&s2))
    })
    .await?
    .map_err(bad)?;
    tracing::info!("DeckLink card {what} by {}", me.username);
    Ok(Json(v))
}

/// Ce qu'une appli publie dans un partage de sortie : qui, quel format, quel timecode.
fn published(path: &Path, s: &crate::state::AppState) -> Value {
    let Ok(r) = strata_share::Reader::observe(path) else {
        return json!({"sending": false, "share": path});
    };
    if r.gone() {
        return json!({"sending": false, "share": path});
    }
    let pid = r.writer_pid();
    // L'appli : STRATA_APP_ID dans l'environnement du processus (lancé par le Core).
    let app = std::fs::read(format!("/proc/{pid}/environ"))
        .ok()
        .and_then(|env| env.split(|&b| b == 0).find_map(|kv| kv.strip_prefix(b"STRATA_APP_ID=").map(|v| String::from_utf8_lossy(v).into_owned())));
    let name = app.as_deref().and_then(|a| s.apps.get(a)).map(|a| a.manifest.name);
    let f = r.info();
    json!({
        "sending": true,
        "share": path,
        "app": app,
        "appName": name,
        "format": f.map(|f| capture::format_name(&f.info)),
        "timecode": f.and_then(|f| f.info.timecode).map(|t| capture::tc_text(&t)),
    })
}

/// Les sorties SDI réglées sur les cartes présentes, et ce que les applis y envoient.
async fn outputs(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let s2 = s.clone();
    let v = blocking(move || {
        let cfg = HardwareConfig::load(&s2.data_dir);
        let (detected, _, _) = decklink::detect();
        let mut out = Vec::new();
        for c in &cfg.cards {
            let Some(d) = detected.iter().find(|d| d.id == c.id) else { continue };
            for (i, k) in c.connectors.iter().enumerate() {
                if k.function != Function::Output || d.ports.get(i) == Some(&decklink::Port::In) {
                    continue;
                }
                let mut v = match mxl_of(&s2) {
                    Some(m) => published_mxl(&m, out_flow(&format!("sdi-out/{}/{}", c.id, i + 1))),
                    None => published(&strata_share::sdi_out_path(&c.id, i as u32 + 1), &s2),
                };
                v["card"] = json!(c.id);
                v["connector"] = json!(i + 1);
                // Avec MXL, le Core tient la sortie : ce qui part sur le câble.
                v["playout"] = json!(s2.playouts.status_of(&c.id, i as u32 + 1));
                out.push(v);
            }
        }
        json!({ "outputs": out })
    })
    .await?;
    Ok(Json(v))
}

async fn output_preview(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath((card, n)): UrlPath<(String, u32)>) -> ApiResult<impl IntoResponse> {
    require(me.admin)?;
    let s2 = s.clone();
    let key = format!("out/{card}/{n}");
    let jpeg = blocking(move || {
        s2.previews.jpeg(&key, || match mxl_of(&s2) {
            Some(mxl) => preview::Feed::Mxl { mxl, id: out_flow(&format!("sdi-out/{card}/{n}")), reader: None },
            None => preview::Feed::share(strata_share::sdi_out_path(&card, n)),
        })
    })
    .await?
    .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    Ok(jpeg_response(jpeg))
}

/// Les pattes d'un flux reçu : le réseau rouge par le port média 1 (ou le port choisi), le bleu
/// par le port 2.
fn legs(cfg: &HardwareConfig, f: &Flow) -> Vec<st2110::Leg> {
    let ip = |name: &str| net::interface_ip(name).unwrap_or(Ipv4Addr::UNSPECIFIED);
    let r = &cfg.network;
    match (r.media_mode, f.blue) {
        (MediaMode::Redundant, Some(blue)) => {
            vec![st2110::Leg { group: f.red, iface: ip(&r.media1), name: "red" }, st2110::Leg { group: blue, iface: ip(&r.media2), name: "blue" }]
        }
        (MediaMode::Redundant, None) => vec![st2110::Leg { group: f.red, iface: ip(&r.media1), name: "red" }],
        (MediaMode::Separate, _) => {
            vec![st2110::Leg { group: f.red, iface: ip(if f.port == 2 { &r.media2 } else { &r.media1 }), name: if f.port == 2 { "port2" } else { "port1" } }]
        }
    }
}

/// La clé de l'aperçu d'un flux : change avec ses adresses, pour qu'un flux modifié soit
/// rejoint à nouveau.
fn flow_key(cfg: &HardwareConfig, f: &Flow) -> String {
    let legs: Vec<String> = legs(cfg, f).iter().map(|l| format!("{}@{}", l.group, l.iface)).collect();
    format!("flow/{}/{:?}/{}", f.id, f.direction, legs.join(","))
}

/// Les flux ST 2110 : pour un flux sortant, ce que l'appli y envoie ; pour un flux entrant
/// regardé, la réception de l'aperçu.
async fn flows(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let s2 = s.clone();
    let v = blocking(move || {
        let cfg = HardwareConfig::load(&s2.data_dir);
        let list: Vec<Value> = cfg
            .flows
            .iter()
            .map(|f| {
                let mut v = match f.direction {
                    // Tenu par le Core (MTL) : ce qui arrive ou part vraiment.
                    _ if s2.st2110.active() && f.essence != Essence::Data => json!({ "io": s2.st2110.status_of(&f.id) }),
                    Direction::Output => match mxl_of(&s2) {
                        Some(m) => published_mxl(&m, out_flow(&format!("2110/{}", f.id))),
                        None => published(&strata_share::flow_path(&f.id), &s2),
                    },
                    Direction::Input => json!({ "reception": s2.previews.flow_status(&flow_key(&cfg, f)) }),
                };
                v["id"] = json!(f.id);
                v
            })
            .collect();
        json!({ "flows": list, "io": { "active": s2.st2110.active(), "problem": s2.st2110.problem() } })
    })
    .await?;
    Ok(Json(v))
}

/// L'aperçu d'un flux vidéo : publié par l'appli qui l'envoie, ou reçu pendant qu'on le regarde.
async fn flow_preview(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath(id): UrlPath<String>) -> ApiResult<impl IntoResponse> {
    require(me.admin)?;
    let s2 = s.clone();
    let jpeg = blocking(move || {
        let cfg = HardwareConfig::load(&s2.data_dir);
        let f = cfg.flows.iter().find(|f| f.id == id).ok_or("no such flow")?;
        if f.essence != Essence::Video {
            return Err("only video flows have a picture".to_string());
        }
        let key = flow_key(&cfg, f);
        // Le Core tient le flux (MTL) : son flux MXL, reçu ou écrit par l'appli.
        if let (true, Some(mxl)) = (s2.st2110.active(), mxl_of(&s2)) {
            let key = format!("{key}/mxl");
            return s2.previews.jpeg(&key, || preview::Feed::Mxl { mxl, id: out_flow(&format!("2110/{}", f.id)), reader: None });
        }
        match f.direction {
            Direction::Output => s2.previews.jpeg(&key, || match mxl_of(&s2) {
                Some(mxl) => preview::Feed::Mxl { mxl, id: out_flow(&format!("2110/{}", f.id)), reader: None },
                None => preview::Feed::share(strata_share::flow_path(&f.id)),
            }),
            Direction::Input => {
                let legs = legs(&cfg, f);
                s2.previews.jpeg(&key, || preview::Feed::Flow(st2110::Receiver::start(legs)))
            }
        }
    })
    .await?
    .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    Ok(jpeg_response(jpeg))
}

async fn read(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let s2 = s.clone();
    Ok(Json(blocking(move || snapshot(&s2)).await?))
}

async fn write(State(s): State<Shared>, CurrentUser(me): CurrentUser, Json(mut cfg): Json<HardwareConfig>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let s2 = s.clone();
    let saved = blocking(move || {
        let (cards, _, _) = decklink::detect();
        cfg.check(&cards)?;
        let before = HardwareConfig::load(&s2.data_dir);
        strata_core::files::write_json(&HardwareConfig::path(&s2.data_dir), &cfg, false).map_err(|e| format!("cannot save the hardware settings: {e}"))?;
        reconcile_inputs(&s2);
        let ndi_changed = ndi::write_settings(&s2.data_dir, &cfg.ndi, &cfg.network.ndi);
        // L'heure : remise en place quand ce qui la règle change.
        let files = clock::files(&cfg);
        let applied = (files != clock::files(&before)).then(|| clock::apply(&s2.data_dir, &files, false));
        Ok::<_, String>((snapshot(&s2), ndi_changed, applied))
    })
    .await?
    .map_err(bad)?;
    tracing::info!("hardware settings changed by {}", me.username);
    let (mut v, ndi_changed, applied) = saved;
    v["ndiChanged"] = json!(ndi_changed);
    v["clockApplied"] = json!(applied);
    Ok(Json(v))
}

/// L'horloge : l'état de NTP et de PTP (lu avec leurs outils, quelques secondes au plus), le
/// timecode maison, le bilan, les fichiers que le Core écrit pour le système.
async fn clock(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let s2 = s.clone();
    let v = blocking(move || {
        let cfg = HardwareConfig::load(&s2.data_dir);
        s2.clock.set_domain(cfg.ptp.domain);
        let st = s2.clock.read();
        let tz = s2.config.load().timezone.clone();
        let (cards, _, _) = decklink::detect();
        let checks = clock::checks(&cfg, &st, &cards, &s2.captures.list(), &tz);
        let mut v = serde_json::to_value(&st).unwrap_or_default();
        v["house"] = clock::house_now(&cfg, &tz);
        v["timezone"] = json!(tz);
        v["checks"] = json!(checks);
        v["files"] = json!(clock::files(&cfg));
        v
    })
    .await?;
    Ok(Json(v))
}

/// Remet en place les fichiers de l'heure et relance chrony (et ptp4l).
async fn clock_apply(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let s2 = s.clone();
    let steps = blocking(move || clock::apply(&s2.data_dir, &clock::files(&HardwareConfig::load(&s2.data_dir)), true)).await?;
    tracing::info!("clock settings applied by {}", me.username);
    Ok(Json(json!({ "steps": steps })))
}

/// Les sources NDI vues sur le réseau.
async fn ndi_sources(CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let list = blocking(|| ndi::sources(3000)).await?.map_err(bad)?;
    Ok(Json(json!({ "sources": list })))
}

/// Ce que les applis lisent du matériel : réservé à la machine elle-même (les applis, lancées
/// par le Core) et aux administrateurs. Chaque connecteur porte le numéro d'appareil Desktop Video
/// à ouvrir.
async fn shared(State(s): State<Shared>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, MaybeUser(user): MaybeUser) -> ApiResult<Json<Value>> {
    if !(guard::this_machine(peer, &headers) || user.is_some_and(|u| u.admin)) {
        return Err((StatusCode::FORBIDDEN, "only the apps of this server and its administrators".into()));
    }
    let s2 = s.clone();
    let v = blocking(move || {
        let cfg = HardwareConfig::load(&s2.data_dir);
        let (detected, _, _) = decklink::detect();
        let mxl = mxl_of(&s2);
        let sdi: Vec<Value> = cfg
            .cards
            .iter()
            .map(|c| {
                let d = detected.iter().find(|d| d.id == c.id);
                json!({
                    "id": c.id,
                    "name": c.name,
                    "model": d.map_or(c.model_name.as_str(), |d| d.model_name.as_str()),
                    "present": d.is_some(),
                    "genlock": c.genlock,
                    "genlockOffset": c.genlock_offset,
                    "connectors": c.connectors.iter().enumerate().map(|(i, k)| json!({
                        "number": i + 1,
                        "name": k.name,
                        "function": k.function,
                        "device": d.and_then(|d| d.devices.get(i)),
                        "share": match k.function {
                            Function::Input => s2.captures.share_of(&c.id, i as u32 + 1),
                            Function::Output if mxl.is_none() => Some(strata_share::sdi_out_path(&c.id, i as u32 + 1)),
                            _ => None,
                        },
                        // Avec MXL : le flux à lire (entrée) ou à écrire (sortie).
                        "mxlFlow": match k.function {
                            Function::Input => s2.captures.mxl_flow_of(&c.id, i as u32 + 1),
                            Function::Output if mxl.is_some() => Some(out_flow(&format!("sdi-out/{}/{}", c.id, i + 1)).to_string()),
                            _ => None,
                        },
                        // Le son embarqué d'une entrée (float32 48 kHz, les canaux de la carte).
                        "audioFlow": match k.function {
                            Function::Input if mxl.is_some() => Some(out_flow(&format!("sdi/{}/{}/audio", c.id, i + 1)).to_string()),
                            _ => None,
                        },
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        let ip = |name: &str| net::interface_ip(name).map(|ip: Ipv4Addr| ip.to_string());
        let r = &cfg.network;
        json!({
            "sdi": sdi,
            "network": {
                "management": {"interface": r.management, "ip": ip(&r.management)},
                "ndi": {"interface": r.ndi, "ip": ip(&r.ndi)},
                "mediaMode": r.media_mode,
                "media1": {"interface": r.media1, "ip": ip(&r.media1)},
                "media2": {"interface": r.media2, "ip": ip(&r.media2)},
                "ptp": {"interface": r.ptp, "ip": ip(&r.ptp)},
            },
            "ndi": {"configDir": ndi::config_dir(&s2.data_dir), "discovery": cfg.ndi.discovery, "machine": cfg.ndi.machine},
            // Un flux sortant : où l'appli qui l'envoie publie aussi ses images (pour l'aperçu).
            "flows": cfg.flows.iter().map(|f| {
                let mut v = json!(f);
                if mxl.is_some() {
                    v["mxlFlow"] = json!(out_flow(&format!("2110/{}", f.id)).to_string());
                } else if f.direction == Direction::Output {
                    v["share"] = json!(strata_share::flow_path(&f.id));
                }
                v
            }).collect::<Vec<_>>(),
            "ptp": cfg.ptp,
            // Les GPU : où encoder, décoder ou rendre.
            "gpus": gpu::detect().into_iter().map(|g| json!({"pci": g.pci, "vendor": g.vendor, "name": g.name, "driver": g.driver, "renderNode": g.render_node, "api": g.api, "encode": g.encode, "decode": g.decode, "ready": g.ready})).collect::<Vec<_>>(),
            "clock": {"source": cfg.clock.source, "house": cfg.clock.house, "timezone": s2.config.load().timezone},
            // MXL : le domaine où sont les flux (null : pas de MXL, le partage strata-share).
            "mxl": mxl.as_ref().map(|m| json!({"domain": m.domain(), "version": m.version})),
            // Les flux réseau tenus par le Core : entrées à lire, sorties à écrire.
            "streams": crate::streams::api::shared(&s2),
        })
    })
    .await?;
    Ok(Json(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flow(name: &str, red: &str, blue: Option<&str>) -> Flow {
        Flow {
            id: String::new(),
            name: name.into(),
            direction: Direction::Output,
            essence: Essence::Video,
            red: red.parse().unwrap(),
            blue: blue.map(|b| b.parse().unwrap()),
            port: 1,
            format: String::new(),
            payload_type: 96,
            channels: 2,
            bits: 24,
            ptime_us: 1000,
            enabled: true,
        }
    }

    #[test]
    fn video_formats_by_name() {
        assert_eq!(parse_format("1080i50"), Some((1920, 1080, (25, 1), true)));
        assert_eq!(parse_format("1080p50"), Some((1920, 1080, (50, 1), false)));
        assert_eq!(parse_format("1080i59.94"), Some((1920, 1080, (30000, 1001), true)));
        assert_eq!(parse_format("2160p59.94"), Some((3840, 2160, (60000, 1001), false)));
        assert_eq!(parse_format("720p50"), Some((1280, 720, (50, 1), false)));
        assert_eq!(parse_format("1080p23.98"), Some((1920, 1080, (24000, 1001), false)));
        assert_eq!(parse_format("abc"), None);
    }

    #[test]
    fn checks() {
        let cards = decklink::group(&decklink::fake_devices(&["minimonitor".into(), "duo2".into()]));
        let (mini, duo) = (&cards[0], &cards[1]);
        let mut hw = HardwareConfig {
            cards: vec![Card {
                id: duo.id.clone(),
                name: " Plateau ".into(),
                connectors: vec![Connector { name: "Caméra 1".into(), function: Function::Input }],
                ..Card::default()
            }],
            flows: vec![flow("Régie", "239.1.0.1:5004", Some("239.2.0.1:5004")), flow("Son", "239.1.0.2:5004", Some("239.2.0.2:5004"))],
            ..HardwareConfig::default()
        };
        hw.check(&cards).unwrap();
        assert_eq!(hw.cards[0].name, "Plateau");
        assert_eq!((hw.flows[0].id.as_str(), hw.flows[1].id.as_str()), ("flow-1", "flow-2"));

        // Une Mini Monitor ne fait que sortir.
        let mut bad = hw.clone();
        bad.cards.push(Card { id: mini.id.clone(), connectors: vec![Connector { function: Function::Input, ..Connector::default() }], ..Card::default() });
        assert!(bad.check(&cards).unwrap_err().contains("cannot be an input"));
        let mut bad = hw.clone();
        bad.flows.push(flow("Double", "239.2.0.1:5004", Some("239.2.0.9:5004")));
        assert_eq!(bad.check(&cards).unwrap_err(), "239.2.0.1:5004 is used by two flows");
        let mut bad = hw.clone();
        bad.flows.push(flow("Unicast", "10.0.0.1:5004", Some("239.2.0.8:5004")));
        assert!(bad.check(&cards).unwrap_err().contains("must be multicast"));
        let mut bad = hw.clone();
        bad.network = NetworkRoles { media1: "ens2f0".into(), media2: "ens2f0".into(), ..NetworkRoles::default() };
        assert!(bad.check(&cards).is_err());
        // 2022-7 : une adresse sur chaque réseau.
        let mut bad = hw.clone();
        bad.flows.push(flow("Seul", "239.1.0.9:5004", None));
        assert!(bad.check(&cards).unwrap_err().contains("2022-7"));
        // Deux ports indépendants : une seule adresse, par le port choisi.
        let mut sep = hw.clone();
        sep.network.media_mode = MediaMode::Separate;
        sep.flows[1].port = 2;
        sep.check(&cards).unwrap();
        assert_eq!((sep.flows[0].blue, sep.flows[1].port), (None, 2));
        sep.flows[0].port = 3;
        assert!(sep.check(&cards).unwrap_err().contains("1 or 2"));
        let mut bad = hw.clone();
        bad.ptp.domain = 200;
        assert!(bad.check(&cards).is_err());
        let mut bad = hw;
        bad.ndi.discovery = "10.0.0.2; rm -rf".into();
        assert!(bad.check(&cards).is_err());
    }
}
