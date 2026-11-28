//! NMOS : les flux ST 2110 du Core vus et branchés par un contrôleur (Bitfocus Buttons…), sans
//! registre.
//!
//! - **IS-04 en pair-à-pair** : le Core s'annonce en mDNS (`_nmos-node._tcp`, sur l'interface
//!   de gestion), avec les compteurs `ver_*` qui disent au contrôleur quoi relire. Son API Node
//!   (v1.2 et v1.3) décrit le nœud, un appareil, et pour chaque flux ST 2110 réglé : un émetteur
//!   (source, flux, émetteur) ou un récepteur.
//! - **IS-05** (`is05.rs`) : le contrôleur branche nos récepteurs sur ses émetteurs (adresses ou
//!   SDP, tout de suite ou à une heure donnée). Les adresses de nos émetteurs restent celles réglées
//!   à la main : NMOS les lit (SDP) et peut seulement les couper ou les rallumer.
//! - L'API a son propre port (3212 par défaut), sans connexion, comme NMOS le veut : elle ne doit
//!   être joignable que depuis le réseau de la régie.
//!
//! Un branchement est écrit dans les réglages (`hardware.json`) : il tient après un redémarrage,
//! et l'écran ST 2110 le montre.
pub mod is05;
pub mod sdp;

use axum::{
    body::Bytes,
    extract::State,
    http::{header, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    net::{Ipv4Addr, SocketAddrV4},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

use crate::{
    hardware::{net, Direction, Essence, Flow, HardwareConfig, MediaMode},
    state::{AppState, Shared},
};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct NmosSettings {
    pub enabled: bool,
    /// Le port de l'API NMOS (IS-04 et IS-05).
    pub port: u16,
    /// L'interface où le Core s'annonce et répond ; vide : celle de gestion.
    pub interface: String,
    /// Les contrôleurs autorisés (adresses ou réseaux, `10.0.0.0/24`) ; vide : tous.
    pub allowed: Vec<String>,
    /// Les pages web autorisées à brancher (contrôleur dans un navigateur,
    /// `http://10.0.0.5:8080`) ; une page d'une autre origine ne peut que lire.
    pub origins: Vec<String>,
}

impl Default for NmosSettings {
    fn default() -> Self {
        NmosSettings { enabled: false, port: 3212, interface: String::new(), allowed: Vec::new(), origins: Vec::new() }
    }
}

/// Gardé dans `nmos.json` : l'identité du nœud, et par flux ce que NMOS y a branché.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Saved {
    pub node_id: Option<Uuid>,
    pub links: HashMap<String, Link>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Link {
    /// L'émetteur branché sur notre récepteur (ou le récepteur qui suit notre émetteur).
    pub peer: Option<Uuid>,
    /// Par patte : l'adresse de l'émetteur (récepteur).
    pub sources: Vec<Option<Ipv4Addr>>,
    /// Le SDP reçu au branchement (récepteur).
    pub sdp: Option<String>,
}

impl Saved {
    fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("nmos.json")
    }

    /// Lu, avec une identité de nœud faite (et gardée) la première fois.
    pub fn load(data_dir: &Path) -> Saved {
        let mut s: Saved = strata_core::files::read_json(&Self::path(data_dir)).unwrap_or_default();
        if s.node_id.is_none() {
            s.node_id = Some(Uuid::new_v4());
            s.save(data_dir);
        }
        s
    }

    pub fn save(&self, data_dir: &Path) {
        if let Err(e) = strata_core::files::write_json(&Self::path(data_dir), self, false) {
            tracing::warn!("NMOS: cannot save {}: {e}", Self::path(data_dir).display());
        }
    }

    pub fn node(&self) -> Uuid {
        self.node_id.unwrap_or_default()
    }
}

/// L'état de NMOS dans le Core.
#[derive(Default)]
pub struct Nmos {
    inner: Mutex<Inner>,
    /// Un `apply` à la fois (le gardien et l'enregistrement du matériel).
    applying: tokio::sync::Mutex<()>,
}

#[derive(Default)]
pub(crate) struct Inner {
    run: Option<Run>,
    /// Par ressource : l'empreinte de son contenu et sa version (« secondes:nanosecondes »).
    versions: HashMap<Uuid, (u64, String)>,
    /// Par sorte de ressource (`ver_src`…) : l'empreinte de l'ensemble et le compteur annoncé.
    counters: HashMap<&'static str, (u64, u8)>,
    pub(crate) staged: HashMap<Uuid, Value>,
    /// Les activations programmées (None : réservée, sa tâche pas encore lancée).
    pub(crate) pending: HashMap<Uuid, Option<tokio::task::JoinHandle<()>>>,
    /// La dernière activation de chaque émetteur ou récepteur (pour `active`).
    pub(crate) activations: HashMap<Uuid, Value>,
    error: Option<String>,
}

struct Run {
    key: Key,
    /// Qui peut appeler, et quelles pages web peuvent brancher.
    allowed: Vec<(std::net::IpAddr, u8)>,
    origins: Vec<String>,
    server: tokio::task::JoinHandle<()>,
    mdns: Option<mdns_sd::ServiceDaemon>,
    fullname: String,
    announced: Vec<(String, String)>,
}

/// Ce qui fait relancer l'API et l'annonce.
#[derive(Clone, PartialEq, Debug)]
struct Key {
    port: u16,
    iface: String,
    ip: Ipv4Addr,
    host: String,
}

/// Ce que décrit l'API Node, sans les versions.
pub(crate) struct Model {
    pub node: Value,
    pub devices: Vec<Value>,
    pub sources: Vec<Value>,
    pub flows: Vec<Value>,
    pub senders: Vec<Value>,
    pub receivers: Vec<Value>,
}

/// Un émetteur ou un récepteur, et son flux.
#[derive(Clone, Debug)]
pub(crate) struct End {
    pub id: Uuid,
    pub flow: Flow,
    pub sender: bool,
    /// Ses pattes : l'interface et son adresse (une par réseau en 2022-7).
    pub ifaces: Vec<(String, Option<Ipv4Addr>)>,
}

pub(crate) fn id_of(node: Uuid, kind: &str, flow: &str) -> Uuid {
    Uuid::new_v5(&node, format!("{kind}/{flow}").as_bytes())
}

/// Les émetteurs et récepteurs : les flux vidéo et son réglés.
pub(crate) fn ends(cfg: &HardwareConfig, node: Uuid) -> Vec<End> {
    let r = &cfg.network;
    let ips = cfg.media_ips();
    cfg.flows
        .iter()
        .map(|f| {
            let sender = f.direction == Direction::Output;
            let ifaces = match r.media_mode {
                MediaMode::Redundant => {
                    // Un récepteur sait toujours écouter les deux réseaux, s'il y a deux ports.
                    let two = if sender { f.blue.is_some() } else { !r.media2.is_empty() };
                    let mut v = vec![(r.media1.clone(), ips[0])];
                    if two {
                        v.push((r.media2.clone(), ips[1]));
                    }
                    v
                }
                MediaMode::Separate if f.port == 2 => vec![(r.media2.clone(), ips[1])],
                MediaMode::Separate => vec![(r.media1.clone(), ips[0])],
            };
            End { id: id_of(node, if sender { "sender" } else { "receiver" }, &f.id), flow: f.clone(), sender, ifaces }
        })
        .collect()
}

/// L'heure TAI (PTP, NMOS), d'après celle du système (UTC) et le décalage réglé.
pub(crate) fn tai_now(offset: u32) -> Duration {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default() + Duration::from_secs(offset as u64)
}

pub(crate) fn ts(d: Duration) -> String {
    format!("{}:{}", d.as_secs(), d.subsec_nanos())
}

pub(crate) fn parse_ts(s: &str) -> Option<Duration> {
    let (a, b) = s.split_once(':')?;
    let nanos: u32 = b.parse().ok()?;
    (nanos < 1_000_000_000).then_some(Duration::new(a.parse().ok()?, nanos))
}

/// « ac1f6b.fffe.0a0b0c » (ptp4l) → les 8 octets.
fn clock_id(gm: &str) -> Option<Vec<String>> {
    let hex: String = gm.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    (hex.len() == 16).then(|| (0..8).map(|i| hex[i * 2..i * 2 + 2].to_ascii_lowercase()).collect())
}

/// « 3c:ec:ef:10:00:01 » → « 3c-ec-ef-10-00-01 ».
fn mac_dashed(mac: &str) -> String {
    mac.to_ascii_lowercase().replace(':', "-")
}

/// L'horloge des flux pour un SDP (`a=ts-refclk`).
pub(crate) fn refclk(s: &AppState, cfg: &HardwareConfig, iface: &str) -> String {
    let ptp = s.clock.get().ptp;
    match ptp.grandmaster.as_deref().and_then(clock_id) {
        Some(id) if ptp.locked => format!("ptp=IEEE1588-2008:{}:{}", id.join("-").to_ascii_uppercase(), cfg.ptp.domain),
        _ => match net::nics().into_iter().find(|n| n.name == iface).and_then(|n| n.mac) {
            Some(mac) => format!("localmac={}", mac_dashed(&mac).to_ascii_uppercase()),
            None => "ptp=IEEE1588-2008:traceable".into(),
        },
    }
}

/// Où répond l'API : l'adresse de l'interface choisie (ou de gestion), sinon la première trouvée.
fn address(cfg: &HardwareConfig) -> (String, Ipv4Addr) {
    let iface = if cfg.nmos.interface.trim().is_empty() { cfg.network.management.clone() } else { cfg.nmos.interface.trim().to_string() };
    if let Some(ip) = (!iface.is_empty()).then(|| net::interface_ip(&iface)).flatten() {
        return (iface, ip);
    }
    let first = if_addrs::get_if_addrs().ok().and_then(|l| {
        l.into_iter().find_map(|i| match i.addr {
            if_addrs::IfAddr::V4(a) if !a.ip.is_loopback() => Some((i.name, a.ip)),
            _ => None,
        })
    });
    first.unwrap_or((String::new(), Ipv4Addr::LOCALHOST))
}

fn host_name() -> String {
    let h = gethostname::gethostname().to_string_lossy().to_string();
    let h: String = h.split('.').next().unwrap_or("strata").chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect();
    if h.is_empty() {
        "strata".into()
    } else {
        h
    }
}

/// Les ressources IS-04 d'après les réglages.
pub(crate) fn model(s: &AppState, cfg: &HardwareConfig, saved: &Saved, api: SocketAddrV4) -> Model {
    let node_id = saved.node();
    let device_id = id_of(node_id, "device", "core");
    let label = {
        let n = s.config.load().name.trim().to_string();
        if n.is_empty() {
            host_name()
        } else {
            n
        }
    };
    let base = format!("http://{api}");
    let jpegxs = s.features.on(crate::features::JPEGXS);
    let ptp = s.clock.get().ptp;
    let clock = match ptp.grandmaster.as_deref().and_then(clock_id) {
        Some(id) if ptp.running => {
            json!({ "name": "clk0", "ref_type": "ptp", "traceable": false, "version": "IEEE1588-2008", "gmid": id.join("-"), "locked": ptp.locked })
        }
        _ => json!({ "name": "clk0", "ref_type": "internal" }),
    };
    let nics = net::nics();
    let mut interfaces = Vec::new();
    for name in [&cfg.network.media1, &cfg.network.media2] {
        if name.is_empty() {
            continue;
        }
        let mac = nics.iter().find(|n| &n.name == name).and_then(|n| n.mac.clone()).map(|m| mac_dashed(&m));
        interfaces.push(json!({ "name": name, "chassis_id": mac.clone(), "port_id": mac.unwrap_or_else(|| "00-00-00-00-00-00".into()) }));
    }
    let hostname = {
        let dns = s.config.load().dns_name.trim().to_string();
        if dns.is_empty() {
            host_name()
        } else {
            dns
        }
    };
    let node = json!({
        "id": node_id,
        "label": label,
        "description": "Strata Core",
        "tags": {},
        "href": format!("{base}/"),
        "hostname": hostname,
        "api": { "versions": ["v1.2", "v1.3"], "endpoints": [{ "host": api.ip().to_string(), "port": api.port(), "protocol": "http" }] },
        "caps": {},
        "services": [],
        "clocks": [clock],
        "interfaces": interfaces,
    });
    let (mut sources, mut flows, mut senders, mut receivers) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for e in ends(cfg, node_id) {
        let f = &e.flow;
        let link = saved.links.get(&f.id).cloned().unwrap_or_default();
        let bindings: Vec<&String> = e.ifaces.iter().map(|(n, _)| n).collect();
        let (label, hint) = cfg.nmos_names(f);
        let tags = json!({ "urn:x-nmos:tag:grouphint/v1.0": [hint] });
        let (format, media_types) = match f.essence {
            Essence::Audio => ("urn:x-nmos:format:audio", json!(["audio/L24", "audio/L16"])),
            Essence::Data => ("urn:x-nmos:format:data", json!(["video/smpte291"])),
            // JPEG XS seulement si la fonction est active (Réglages › Fonctions).
            Essence::Video if jpegxs => ("urn:x-nmos:format:video", json!(["video/raw", "video/jxsv"])),
            Essence::Video => ("urn:x-nmos:format:video", json!(["video/raw"])),
        };
        if !e.sender {
            receivers.push(json!({
                "id": e.id,
                "label": label,
                "description": "",
                "tags": tags,
                "device_id": device_id,
                "transport": "urn:x-nmos:transport:rtp.mcast",
                "interface_bindings": bindings,
                "subscription": { "sender_id": link.peer, "active": f.enabled },
                "format": format,
                "caps": { "media_types": media_types },
            }));
            continue;
        }
        let source_id = id_of(node_id, "source", &f.id);
        let flow_id = id_of(node_id, "flow", &f.id);
        let common = |id: Uuid| json!({ "id": id, "label": label, "description": "", "tags": tags, "device_id": device_id, "parents": [] });
        match f.essence {
            Essence::Audio => {
                let mut src = common(source_id);
                src["format"] = json!(format);
                src["clock_name"] = json!("clk0");
                src["caps"] = json!({});
                src["channels"] = json!((1..=f.channels).map(|c| json!({ "label": format!("Ch {c}") })).collect::<Vec<_>>());
                sources.push(src);
                let mut fl = common(flow_id);
                fl["source_id"] = json!(source_id);
                fl["format"] = json!(format);
                fl["media_type"] = json!(format!("audio/L{}", f.bits));
                fl["sample_rate"] = json!({ "numerator": 48000 });
                fl["bit_depth"] = json!(f.bits);
                flows.push(fl);
            }
            Essence::Data => {
                let (_, _, (n, d), _) = cfg.video_of(f).unwrap_or((1920, 1080, (25, 1), true));
                let rate = json!({ "numerator": n, "denominator": d });
                let mut src = common(source_id);
                src["format"] = json!(format);
                src["clock_name"] = json!("clk0");
                src["caps"] = json!({});
                src["grain_rate"] = rate.clone();
                sources.push(src);
                let mut fl = common(flow_id);
                fl["source_id"] = json!(source_id);
                fl["format"] = json!(format);
                fl["grain_rate"] = rate;
                fl["media_type"] = json!("video/smpte291");
                flows.push(fl);
            }
            _ => {
                let (w, h, (n, d), interlaced) = cfg.video_of(f).unwrap_or((1920, 1080, (25, 1), true));
                let rate = json!({ "numerator": n, "denominator": d });
                let mut src = common(source_id);
                src["format"] = json!(format);
                src["clock_name"] = json!("clk0");
                src["caps"] = json!({});
                src["grain_rate"] = rate.clone();
                sources.push(src);
                let mut fl = common(flow_id);
                fl["source_id"] = json!(source_id);
                fl["format"] = json!(format);
                fl["grain_rate"] = rate;
                match f.jpegxs() {
                    Some(bpp) => {
                        fl["media_type"] = json!("video/jxsv");
                        fl["profile"] = json!("Main422.10");
                        fl["bit_rate"] = json!(sdp::jxs_kbps(w, h, (n, d), bpp));
                    }
                    None => fl["media_type"] = json!("video/raw"),
                }
                fl["frame_width"] = json!(w);
                fl["frame_height"] = json!(h);
                fl["interlace_mode"] = json!(if interlaced { "interlaced_tff" } else { "progressive" });
                fl["colorspace"] = json!("BT709");
                fl["transfer_characteristic"] = json!("SDR");
                fl["components"] = json!([
                    { "name": "Y", "width": w, "height": h, "bit_depth": 10 },
                    { "name": "Cb", "width": w / 2, "height": h, "bit_depth": 10 },
                    { "name": "Cr", "width": w / 2, "height": h, "bit_depth": 10 },
                ]);
                flows.push(fl);
            }
        }
        senders.push(json!({
            "id": e.id,
            "label": label,
            "description": "",
            "tags": tags,
            "flow_id": flow_id,
            "transport": "urn:x-nmos:transport:rtp.mcast",
            "device_id": device_id,
            "manifest_href": format!("{base}/x-nmos/connection/v1.1/single/senders/{}/transportfile", e.id),
            "interface_bindings": bindings,
            "subscription": { "receiver_id": link.peer, "active": f.enabled },
        }));
    }
    let device = json!({
        "id": device_id,
        "label": label,
        "description": "Strata Core ST 2110",
        "tags": {},
        "type": "urn:x-nmos:device:generic",
        "node_id": node_id,
        "senders": senders.iter().map(|s| s["id"].clone()).collect::<Vec<_>>(),
        "receivers": receivers.iter().map(|r| r["id"].clone()).collect::<Vec<_>>(),
        "controls": [{ "type": "urn:x-nmos:control:sr-ctrl/v1.1", "href": format!("{base}/x-nmos/connection/v1.1/") }],
    });
    Model { node, devices: vec![device], sources, flows, senders, receivers }
}

fn hash_of(v: &Value) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.to_string().hash(&mut h);
    h.finish()
}

impl Inner {
    /// Donne à chaque ressource sa version (changée avec son contenu), et tient les compteurs
    /// `ver_*` annoncés.
    fn stamp(&mut self, m: &mut Model, tai: u32) {
        let now = ts(tai_now(tai));
        let stamp_list =
            |key: &'static str, list: &mut [Value], versions: &mut HashMap<Uuid, (u64, String)>, counters: &mut HashMap<&'static str, (u64, u8)>| {
                let mut all = std::collections::hash_map::DefaultHasher::new();
                for v in list.iter_mut() {
                    let Some(id) = v["id"].as_str().and_then(|i| i.parse::<Uuid>().ok()) else { continue };
                    let h = hash_of(v);
                    (id, h).hash(&mut all);
                    let e = versions.entry(id).or_insert_with(|| (h, now.clone()));
                    if e.0 != h {
                        *e = (h, now.clone());
                    }
                    v["version"] = json!(e.1);
                }
                let all = all.finish();
                let c = counters.entry(key).or_insert((all, 0));
                if c.0 != all {
                    *c = (all, c.1.wrapping_add(1));
                }
            };
        stamp_list("ver_slf", std::slice::from_mut(&mut m.node), &mut self.versions, &mut self.counters);
        stamp_list("ver_dvc", &mut m.devices, &mut self.versions, &mut self.counters);
        stamp_list("ver_src", &mut m.sources, &mut self.versions, &mut self.counters);
        stamp_list("ver_flw", &mut m.flows, &mut self.versions, &mut self.counters);
        stamp_list("ver_snd", &mut m.senders, &mut self.versions, &mut self.counters);
        stamp_list("ver_rcv", &mut m.receivers, &mut self.versions, &mut self.counters);
    }

    fn txt(&self) -> Vec<(String, String)> {
        let mut v = vec![("api_proto".to_string(), "http".to_string()), ("api_ver".into(), "v1.2,v1.3".into()), ("api_auth".into(), "false".into())];
        for k in ["ver_slf", "ver_src", "ver_flw", "ver_dvc", "ver_snd", "ver_rcv"] {
            v.push((k.into(), self.counters.get(k).map_or(0, |c| c.1).to_string()));
        }
        v
    }
}

impl Nmos {
    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap()
    }

    /// Les ressources du moment, avec leurs versions.
    pub(crate) fn current(&self, s: &AppState) -> Option<(Model, HardwareConfig, Saved, SocketAddrV4)> {
        let cfg = HardwareConfig::load(&s.data_dir);
        let saved = Saved::load(&s.data_dir);
        let mut inner = self.lock();
        let api = inner.run.as_ref().map(|r| SocketAddrV4::new(r.key.ip, r.key.port))?;
        let mut m = model(s, &cfg, &saved, api);
        inner.stamp(&mut m, cfg.ptp.tai_offset);
        Some((m, cfg, saved, api))
    }

    /// Pour la page Réseau : marche-t-il, où, avec quelle erreur ; et les branchements.
    pub fn status(&self, s: &AppState) -> Value {
        let saved = Saved::load(&s.data_dir);
        let inner = self.lock();
        let at = inner.run.as_ref().map(|r| format!("http://{}:{}/x-nmos/", r.key.ip, r.key.port));
        json!({
            "running": inner.run.is_some(),
            "url": at,
            "interface": inner.run.as_ref().map(|r| r.key.iface.clone()),
            "nodeId": saved.node_id,
            "error": inner.error,
            "links": saved.links.iter().map(|(k, l)| (k.clone(), json!({ "peer": l.peer }))).collect::<serde_json::Map<_, _>>(),
        })
    }
}

/// Tient l'API et l'annonce en accord avec les réglages ; remet à jour les compteurs annoncés.
pub async fn apply(s: &Shared) {
    let _one = s.nmos.applying.lock().await;
    let s2 = s.clone();
    let Ok((cfg, (iface, ip))) = tokio::task::spawn_blocking(move || {
        let cfg = HardwareConfig::load(&s2.data_dir);
        let a = address(&cfg);
        (cfg, a)
    })
    .await
    else {
        return;
    };
    // ST 2110 coupé (Réglages › Fonctions) : rien à annoncer.
    let want = (cfg.nmos.enabled && s.features.on(crate::features::ST2110)).then(|| Key { port: cfg.nmos.port, iface, ip, host: host_name() });
    let allowed: Vec<_> = cfg.nmos.allowed.iter().filter_map(|a| crate::snmp::net_of(a)).collect();
    let origins: Vec<String> = cfg.nmos.origins.iter().map(|o| o.trim().trim_end_matches('/').to_ascii_lowercase()).filter(|o| !o.is_empty()).collect();
    let same = s.nmos.lock().run.as_ref().map(|r| r.key.clone()) == want;
    if !same {
        // L'API repart (adresse ou port changés) : les activations programmées restent ; NMOS
        // coupé : elles tombent.
        let old = s.nmos.lock().run.take();
        if let Some(run) = old {
            end_run(run).await;
        }
        let Some(key) = want else {
            cancel_pending(s);
            s.nmos.lock().error = None;
            return;
        };
        // Seulement sur l'interface choisie : l'API n'a pas de mot de passe.
        let listener = match tokio::net::TcpListener::bind((key.ip, key.port)).await {
            Ok(l) => l,
            Err(e) => {
                let why = format!("cannot listen on {}:{}: {e}", key.ip, key.port);
                tracing::warn!("NMOS: {why}");
                s.nmos.lock().error = Some(why);
                return;
            }
        };
        let router = Router::new().fallback(handle).with_state(s.clone());
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await;
        });
        let iface = key.iface.clone();
        let mdns = tokio::task::spawn_blocking(move || {
            let d = mdns_sd::ServiceDaemon::new()?;
            if !iface.is_empty() && net::interface_ip(&iface).is_some() {
                let _ = d.disable_interface(mdns_sd::IfKind::All);
                let _ = d.enable_interface(mdns_sd::IfKind::Name(iface));
            }
            Ok::<_, mdns_sd::Error>(d)
        })
        .await;
        let (mdns, error) = match mdns {
            Ok(Ok(d)) => (Some(d), None),
            Ok(Err(e)) => (None, Some(format!("mDNS: {e}"))),
            Err(e) => (None, Some(format!("mDNS: {e}"))),
        };
        let fullname = format!("strata-{}-{}._nmos-node._tcp.local.", key.host, key.port);
        tracing::info!("NMOS node on http://{}:{} ({})", key.ip, key.port, if key.iface.is_empty() { "all interfaces" } else { &key.iface });
        let mut inner = s.nmos.lock();
        inner.error = error;
        inner.run = Some(Run { key, allowed: allowed.clone(), origins: origins.clone(), server, mdns, fullname, announced: Vec::new() });
    }
    if let Some(run) = s.nmos.lock().run.as_mut() {
        run.allowed = allowed;
        run.origins = origins;
    }
    // Les versions et les compteurs, puis l'annonce si elle a changé.
    let s2 = s.clone();
    let _ = tokio::task::spawn_blocking(move || {
        s2.nmos.current(&s2);
        announce(&s2);
    })
    .await;
}

/// Arrête une API : le serveur fini (son port libéré) avant de rendre la main, l'annonce retirée.
async fn end_run(run: Run) {
    run.server.abort();
    let _ = run.server.await;
    if let Some(d) = run.mdns {
        let name = run.fullname;
        let _ = tokio::task::spawn_blocking(move || unannounce(d, &name)).await;
    }
}

fn unannounce(d: mdns_sd::ServiceDaemon, fullname: &str) {
    if let Ok(rx) = d.unregister(fullname) {
        let _ = rx.recv_timeout(Duration::from_millis(500));
    }
    let _ = d.shutdown();
}

fn cancel_pending(s: &AppState) {
    let mut inner = s.nmos.lock();
    let ids: Vec<Uuid> = inner.pending.keys().copied().collect();
    for id in ids {
        if let Some(Some(t)) = inner.pending.remove(&id) {
            t.abort();
        }
        if let Some(st) = inner.staged.get_mut(&id) {
            st["activation"] = json!({ "mode": null, "requested_time": null, "activation_time": null });
        }
        tracing::warn!("NMOS: the scheduled activation of {id} is cancelled (NMOS turned off)");
    }
}

/// (Re)fait l'annonce mDNS quand ses compteurs ont changé.
pub(crate) fn announce(s: &AppState) {
    let mut inner = s.nmos.lock();
    let txt = inner.txt();
    let Some(run) = inner.run.as_mut() else { return };
    if run.announced == txt {
        return;
    }
    let Some(d) = &run.mdns else { return };
    let instance = run.fullname.split("._nmos-node").next().unwrap_or("strata").to_string();
    let props: Vec<(&str, &str)> = txt.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    match mdns_sd::ServiceInfo::new("_nmos-node._tcp.local.", &instance, &format!("{}.local.", run.key.host), run.key.ip.to_string(), run.key.port, &props[..])
    {
        Ok(info) => match d.register(info) {
            Ok(()) => run.announced = txt,
            Err(e) => tracing::warn!("NMOS: mDNS announce failed: {e}"),
        },
        Err(e) => tracing::warn!("NMOS: mDNS record: {e}"),
    }
}

/// Arrête l'API et retire l'annonce (le contrôleur voit le nœud partir).
/// À l'arrêt du Core (bloquant) : l'API arrêtée, l'annonce retirée (le contrôleur voit le nœud
/// partir).
pub fn stop(s: &AppState) {
    let run = s.nmos.lock().run.take();
    if let Some(run) = run {
        run.server.abort();
        if let Some(d) = run.mdns {
            unannounce(d, &run.fullname);
        }
    }
    let mut inner = s.nmos.lock();
    for (_, t) in inner.pending.drain() {
        if let Some(t) = t {
            t.abort();
        }
    }
}

// ---------- L'API HTTP ----------

pub(crate) enum Reply {
    Json(StatusCode, Value),
    Sdp(String),
}

pub(crate) fn error(code: StatusCode, why: impl ToString) -> Reply {
    Reply::Json(code, json!({ "code": code.as_u16(), "error": why.to_string(), "debug": null }))
}

fn ok(v: Value) -> Reply {
    Reply::Json(StatusCode::OK, v)
}

/// CORS : tout le monde peut lire ; seules les pages des origines autorisées peuvent brancher.
fn cors(mut r: Response, write: bool) -> Response {
    let h = r.headers_mut();
    h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, header::HeaderValue::from_static("*"));
    let methods = if write { "GET, PUT, POST, PATCH, HEAD, OPTIONS, DELETE" } else { "GET, HEAD, OPTIONS" };
    h.insert(header::ACCESS_CONTROL_ALLOW_METHODS, header::HeaderValue::from_static(methods));
    h.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, header::HeaderValue::from_static("Content-Type, Accept"));
    h.insert(header::ACCESS_CONTROL_MAX_AGE, header::HeaderValue::from_static("3600"));
    r
}

fn plain(code: StatusCode, why: &str) -> Response {
    let Reply::Json(code, v) = error(code, why) else { unreachable!() };
    (code, [(header::CONTENT_TYPE, "application/json")], v.to_string()).into_response()
}

async fn handle(
    State(s): State<Shared>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    method: Method,
    uri: Uri,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Response {
    let (allowed, origins) = s.nmos.lock().run.as_ref().map(|r| (r.allowed.clone(), r.origins.clone())).unwrap_or_default();
    if !allowed.is_empty() && !allowed.iter().any(|n| crate::snmp::in_net(peer.ip(), *n)) {
        return plain(StatusCode::FORBIDDEN, "this address may not use the NMOS API");
    }
    // Une page web ne branche que si son origine est autorisée (sinon n'importe quelle page
    // ouverte sur un poste de la régie pourrait couper un émetteur).
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()).map(|o| o.trim_end_matches('/').to_ascii_lowercase());
    let may_write = origin.as_ref().is_none_or(|o| origins.contains(o));
    if method == Method::OPTIONS {
        return cors(StatusCode::OK.into_response(), may_write);
    }
    let segs: Vec<String> = uri.path().split('/').filter(|x| !x.is_empty()).map(str::to_string).collect();
    let head = method == Method::HEAD;
    let reply = if segs.get(1).map(String::as_str) == Some("connection") && method != Method::GET && !head {
        let json =
            headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|c| c.trim().to_ascii_lowercase().starts_with("application/json"));
        if !may_write {
            return cors(plain(StatusCode::FORBIDDEN, "this web page may not change connections"), false);
        }
        if !json {
            return cors(plain(StatusCode::UNSUPPORTED_MEDIA_TYPE, "the body must be application/json"), may_write);
        }
        is05::write(&s, &method, &segs, &body).await
    } else if method == Method::GET || head {
        let s2 = s.clone();
        tokio::task::spawn_blocking(move || get(&s2, &segs)).await.unwrap_or_else(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e))
    } else {
        error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed")
    };
    let r = match reply {
        Reply::Json(code, v) => (code, [(header::CONTENT_TYPE, "application/json")], v.to_string()).into_response(),
        Reply::Sdp(text) => (StatusCode::OK, [(header::CONTENT_TYPE, "application/sdp")], text).into_response(),
    };
    cors(r, may_write)
}

fn names(list: &[&str]) -> Reply {
    ok(json!(list))
}

fn get(s: &AppState, segs: &[String]) -> Reply {
    let p: Vec<&str> = segs.iter().map(String::as_str).collect();
    match p.as_slice() {
        [] => names(&["x-nmos/"]),
        ["x-nmos"] => names(&["node/", "connection/"]),
        ["x-nmos", "node"] => names(&["v1.2/", "v1.3/"]),
        ["x-nmos", "node", v, rest @ ..] if *v == "v1.2" || *v == "v1.3" => node_api(s, rest),
        ["x-nmos", "connection", rest @ ..] => is05::read(s, rest),
        _ => error(StatusCode::NOT_FOUND, "not found"),
    }
}

fn node_api(s: &AppState, rest: &[&str]) -> Reply {
    if rest.is_empty() {
        return names(&["self/", "sources/", "flows/", "devices/", "senders/", "receivers/"]);
    }
    let Some((m, ..)) = s.nmos.current(s) else { return error(StatusCode::SERVICE_UNAVAILABLE, "NMOS is off") };
    let list = match rest[0] {
        "self" if rest.len() == 1 => return ok(m.node),
        "sources" => m.sources,
        "flows" => m.flows,
        "devices" => m.devices,
        "senders" => m.senders,
        "receivers" => m.receivers,
        _ => return error(StatusCode::NOT_FOUND, "not found"),
    };
    match rest {
        [_] => ok(json!(list)),
        [_, id] => match list.into_iter().find(|v| v["id"].as_str() == Some(id)) {
            Some(v) => ok(v),
            None => error(StatusCode::NOT_FOUND, "no such resource"),
        },
        _ => error(StatusCode::NOT_FOUND, "not found"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_and_clock_ids() {
        assert_eq!(parse_ts("1700000000:500"), Some(Duration::new(1_700_000_000, 500)));
        assert_eq!(parse_ts("1:1000000000"), None);
        assert_eq!(ts(Duration::new(5, 7)), "5:7");
        assert_eq!(clock_id("ac1f6b.fffe.0a0b0c").unwrap().join("-"), "ac-1f-6b-ff-fe-0a-0b-0c");
    }
}
