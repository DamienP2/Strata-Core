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
//! - **Entrées** : chaque entrée SDI captée une fois par le Core et partagée avec les applis
//!   (`capture.rs`, `strata-share`).
//!
//! Gardé dans `hardware.json`. Les applis le lisent par `GET /api/hardware/shared` (docs/APPLIS.md).
pub mod capture;
pub mod decklink;
pub mod ndi;
pub mod net;
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
fn snapshot(s: &Shared) -> Value {
    let cfg = HardwareConfig::load(&s.data_dir);
    let (cards, version, fake) = decklink::detect();
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
        "ndi": ndi,
    })
}

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/hardware", get(read).put(write))
        .route("/api/hardware/clock", get(clock))
        .route("/api/hardware/ndi/sources", get(ndi_sources))
        .route("/api/hardware/shared", get(shared))
        .route("/api/hardware/inputs", get(inputs))
        .route("/api/hardware/inputs/{card}/{n}/preview.jpg", get(preview))
}

/// Tient les captures en accord avec les réglages et les cartes présentes (bloquant).
pub fn reconcile_inputs(s: &crate::state::AppState) {
    let cfg = HardwareConfig::load(&s.data_dir);
    let (detected, _, simulated) = decklink::detect();
    s.captures.reconcile(capture::wanted(&cfg, &detected, simulated));
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

/// L'image la plus récente d'une entrée, en petit (JPEG).
async fn preview(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath((card, n)): UrlPath<(String, u32)>) -> ApiResult<impl IntoResponse> {
    require(me.admin)?;
    let share = s.captures.share_of(&card, n).ok_or((StatusCode::NOT_FOUND, "this input is not captured".to_string()))?;
    let jpeg = blocking(move || capture::preview(&share, 480)).await?.map_err(|e| (StatusCode::NOT_FOUND, e))?;
    Ok(([(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "no-store")], jpeg))
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
        strata_core::files::write_json(&HardwareConfig::path(&s2.data_dir), &cfg, false).map_err(|e| format!("cannot save the hardware settings: {e}"))?;
        reconcile_inputs(&s2);
        let ndi_changed = ndi::write_settings(&s2.data_dir, &cfg.ndi, &cfg.network.ndi);
        Ok::<_, String>((snapshot(&s2), ndi_changed))
    })
    .await?
    .map_err(bad)?;
    tracing::info!("hardware settings changed by {}", me.username);
    let (mut v, ndi_changed) = saved;
    v["ndiChanged"] = json!(ndi_changed);
    Ok(Json(v))
}

/// L'état de NTP et de PTP (lu avec leurs outils, quelques secondes au plus).
async fn clock(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<sync::SyncStatus>> {
    require(me.admin)?;
    let domain = HardwareConfig::load(&s.data_dir).ptp.domain;
    s.clock.set_domain(domain);
    let s2 = s.clone();
    Ok(Json(blocking(move || s2.clock.read()).await?))
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
                        "share": s2.captures.share_of(&c.id, i as u32 + 1),
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
            "flows": cfg.flows,
            "ptp": cfg.ptp,
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
        }
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
