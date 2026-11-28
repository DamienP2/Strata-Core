//! Les flux ST 2110 reçus et envoyés par le Core, vidéo (2110-20) et son (2110-30), avec Intel
//! Media Transport Library (MTL) sur les ports média, et MXL pour les applis :
//!
//! - **Reçu** : MTL reçoit le flux (une ou deux pattes, 2022-7) et le Core l'écrit en flux MXL
//!   `2110/<flux>` ; les applis le lisent comme une entrée SDI.
//! - **Envoyé** : l'appli écrit le flux MXL `2110/<flux>` ; le Core l'envoie avec MTL (cadencé
//!   ST 2110-21) ; du noir quand rien n'arrive.
//! - Le mode des ports (Réglages › Réseau) : socket du noyau, AF_XDP ou DPDK. « Off » : les flux
//!   sont seulement réglés (et regardés dans l'aperçu), comme avant.
//! - **Son** : reçu en blocs de 10 ms (L24 ou L16), écrit en flux audio MXL `2110/<flux>` (float32
//!   48 kHz) ; envoyé depuis le flux audio MXL que l'appli écrit, du silence sans lui.
//! - MTL s'ouvre une fois sur les ports réglés et reste ouvert (DPDK ne se réinitialise pas dans
//!   le même processus : des ports changés demandent un redémarrage du Core) ; un flux changé,
//!   ajouté, coupé ou retiré est relancé seul, sans couper les autres (un branchement NMOS ne
//!   touche que le récepteur branché).
use serde::Serialize;
use std::{
    collections::HashMap,
    net::Ipv4Addr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use strata_mtl::{Leg, Mtl, Pmd, Port};
use strata_mxl::Mxl;

use super::{net, Direction, Essence, HardwareConfig, MediaIo, MediaMode};

#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum IoState {
    #[default]
    Opening,
    /// Des images arrivent (reçu) ou partent (envoyé).
    Running,
    /// Rien n'arrive (reçu), ou aucune appli n'écrit (envoyé : du noir part).
    Idle,
    Error,
}

/// Où en est un flux tenu par le Core.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct IoStatus {
    pub state: IoState,
    pub format: Option<String>,
    pub frames: u64,
    /// Reçu : paquets acceptés, manquants sur une patte (avant 2022-7), perdus pour de bon.
    pub packets: u64,
    pub lost: u64,
    pub unrecovered: u64,
    /// Reçu, par patte (rouge, bleue) : paquets reçus, manquants.
    pub legs: Vec<LegIo>,
    pub error: Option<String>,
}

#[derive(Serialize, Clone, Copy, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct LegIo {
    pub packets: u64,
    pub lost: u64,
}

impl IoStatus {
    fn count(&mut self, st: strata_mtl::RxStats, legs: usize) {
        self.packets = st.received;
        self.lost = st.lost;
        self.unrecovered = st.unrecovered;
        self.legs = st.legs.iter().take(legs).map(|&(packets, lost)| LegIo { packets, lost }).collect();
    }
}

/// Ce qui fait ouvrir MTL : les ports et les flux. Un changement rouvre tout.
#[derive(Debug, Clone, PartialEq)]
struct Plan {
    ports: Vec<(String, Pmd, Ipv4Addr, Ipv4Addr)>,
    flows: Vec<FlowPlan>,
    tai_offset: u32,
    /// Les flux mal réglés (format inconnu) : leur erreur, sans toucher aux autres.
    bad: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
struct FlowPlan {
    id: String,
    name: String,
    input: bool,
    legs: Vec<(usize, Ipv4Addr, u16, Option<Ipv4Addr>)>,
    payload_type: u8,
    kind: Kind,
    pacing: strata_mtl::Pacing,
    /// Vidéo JPEG XS (ST 2110-22) : ses bits par pixel.
    jpegxs: Option<f32>,
    /// Envoyé en IPMX : ses rapports RTCP.
    ipmx: Option<IpmxPlan>,
}

/// Les rapports RTCP d'un flux envoyé en IPMX (VSF TR-10-1 § 8.7).
#[derive(Debug, Clone, PartialEq)]
struct IpmxPlan {
    ssrc: u32,
    /// Le bloc d'information IPMX (fixe tant que le flux ne change pas).
    block: Vec<u8>,
    /// Par patte : l'adresse de l'interface du port média, l'adresse et le port du flux.
    legs: Vec<(Ipv4Addr, std::net::SocketAddrV4)>,
    /// Le décalage TAI − UTC (s) : l'heure interne est TAI.
    tai_offset: u32,
    /// Octets et paquets RTP d'une image (ou d'un bloc de son), pour les compteurs du rapport.
    packets: u32,
    octets: u32,
}

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Video(strata_mtl::Video),
    Audio(strata_mtl::Audio),
    /// Données annexes (2110-40), au rythme de cette vidéo.
    Data(strata_mtl::Video),
}

struct Running {
    /// Les ports et le décalage TAI avec lesquels MTL est ouvert.
    ports: (Vec<(String, Pmd, Ipv4Addr, Ipv4Addr)>, u32),
    flows: HashMap<String, FlowRun>,
    // Lâché après les fils : ses sessions sont fermées par eux.
    mtl: Arc<Mtl>,
}

struct FlowRun {
    plan: FlowPlan,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
    started: Instant,
}

/// MTL a déjà été ouvert dans ce processus (DPDK ne se réinitialise pas).
static OPENED: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
pub struct Engine {
    running: Mutex<Option<Running>>,
    status: Arc<Mutex<HashMap<String, IoStatus>>>,
    /// Pourquoi rien ne tourne (bibliothèque absente, ports mal réglés…).
    problem: Mutex<Option<String>>,
}

/// Le nom d'un port pour MTL, son adresse et son masque.
fn port_of(io: MediaIo, iface: &str, address: &str) -> Result<(String, Pmd, Ipv4Addr, Ipv4Addr), String> {
    if iface.is_empty() {
        return Err("choose the media ports in Network".into());
    }
    let cidr = |s: &str| -> Option<(Ipv4Addr, Ipv4Addr)> {
        let (ip, bits) = s.split_once('/').unwrap_or((s, "24"));
        let bits: u32 = bits.parse().ok().filter(|b| *b <= 32)?;
        let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
        Some((ip.trim().parse().ok()?, Ipv4Addr::from(mask)))
    };
    let nic = net::nics().into_iter().find(|n| n.name == iface);
    match io {
        MediaIo::Dpdk => {
            // Déjà en DPDK, le noyau ne le voit plus : son adresse PCI est gardée.
            let pci = super::media::pci_of(iface, &super::media::read_wanted()).ok_or(format!("{iface}: its PCI address is unknown"))?;
            let (ip, mask) = cidr(address).ok_or(format!("{iface}: give its address (10.10.1.21/24), the kernel no longer sees it in DPDK"))?;
            Ok((pci, Pmd::Dpdk, ip, mask))
        }
        MediaIo::Kernel | MediaIo::AfXdp => {
            let (ip, mask) = nic.as_ref().and_then(|n| n.ipv4.first().and_then(|c| cidr(c))).ok_or(format!("{iface} has no IPv4 address"))?;
            let (prefix, pmd) = if io == MediaIo::Kernel { ("kernel", Pmd::Kernel) } else { ("native_af_xdp", Pmd::AfXdp) };
            Ok((format!("{prefix}:{iface}"), pmd, ip, mask))
        }
        MediaIo::Off => Err("off".into()),
    }
}

/// Ce qu'il faut ouvrir d'après les réglages ; None : rien (mode « Off », aucun flux vidéo ou son).
/// L'envoi IPMX d'un flux : l'instant TAI de chaque image (ou bloc de son), qui devient son
/// horodatage RTP, et son rapport RTCP envoyé juste avant elle (pendant l'image précédente).
struct IpmxSend {
    rep: Option<super::ipmx::Reporter>,
    /// Pourquoi les rapports ne partent pas (port média en DPDK : le noyau n'a pas son adresse).
    error: Option<String>,
    period_ns: u64,
    next: u64,
    tai_offset_ns: u64,
    rate: u64,
    packets: u32,
    octets: u32,
}

impl IpmxSend {
    fn open(p: &IpmxPlan, period_ns: u64, rate: u64) -> IpmxSend {
        let (rep, error) = match super::ipmx::Reporter::open(&p.legs, p.ssrc, p.block.clone()) {
            Ok(r) => (Some(r), None),
            Err(e) => (None, Some(e)),
        };
        IpmxSend {
            rep,
            error,
            period_ns: period_ns.max(1),
            next: 0,
            tai_offset_ns: p.tai_offset as u64 * 1_000_000_000,
            rate,
            packets: p.packets,
            octets: p.octets,
        }
    }

    fn now(&self) -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64) + self.tai_offset_ns
    }

    /// L'instant de la prochaine image : son rapport part maintenant (pas plus d'une image avant
    /// elle) ; en retard ou trop en avance, le compte repart de l'heure.
    fn next(&mut self) -> u64 {
        let p = self.period_ns;
        let now = self.now();
        if self.next == 0 || self.next < now || self.next > now + 2_000_000_000 {
            self.next = (now / p + 1) * p;
        }
        // Le rapport de l'image k part pendant l'image k − 1 (TR-10-1 § 8.8.2).
        if self.next > now + p {
            std::thread::sleep(Duration::from_nanos(self.next - p - now));
        }
        let ts = self.next;
        self.next += p;
        if let Some(r) = self.rep.as_mut() {
            r.report(ts, super::ipmx::rtp_of(ts, self.rate), self.packets, self.octets);
        }
        ts
    }
}

/// Le SSRC d'un flux : stable (d'après son identifiant), jamais 0.
fn ssrc_of(id: &str) -> u32 {
    let d = ring::digest::digest(&ring::digest::SHA256, id.as_bytes());
    u32::from_be_bytes(d.as_ref()[..4].try_into().unwrap()) | 1
}

/// `ipmx` : l'horloge de référence à annoncer (`ts-refclk`) quand IPMX est actif.
fn plan(cfg: &HardwareConfig, ipmx: Option<&str>) -> Result<Option<Plan>, String> {
    let r = &cfg.network;
    if r.media_io == MediaIo::Off {
        return Ok(None);
    }
    let video: Vec<_> = cfg.flows.iter().filter(|f| f.enabled && !(f.direction == Direction::Input && f.unset())).collect();
    if video.is_empty() && r.media1.is_empty() {
        return Ok(None);
    }
    // Les deux ports réglés sont ouverts, que les flux du moment s'en servent ou non : MTL (DPDK)
    // ne se rouvre pas sans redémarrer le Core, couper ou ajouter un flux ne doit pas changer ses
    // ports.
    let two = r.media_mode == MediaMode::Redundant && video.iter().any(|f| f.blue.is_some()) || video.iter().any(|f| f.port == 2);
    let mut ports = vec![port_of(r.media_io, &r.media1, &r.media1_address)?];
    if !r.media2.is_empty() {
        match port_of(r.media_io, &r.media2, &r.media2_address) {
            Ok(p) => ports.push(p),
            Err(e) if two => return Err(e),
            Err(_) => {}
        }
    }
    let mut flows = Vec::new();
    let mut bad = Vec::new();
    for f in video {
        let kind = if f.essence == Essence::Audio {
            Kind::Audio(strata_mtl::Audio { channels: f.channels.clamp(1, 64), bits: f.bits, ptime_us: f.ptime_us, frame_ms: 10 })
        } else {
            let Some((width, height, rate, interlaced)) = cfg.video_of(f) else {
                bad.push((f.id.clone(), format!("unknown format {}", f.format)));
                continue;
            };
            let v = strata_mtl::Video { width, height, rate, interlaced };
            if f.essence == Essence::Data {
                Kind::Data(v)
            } else {
                Kind::Video(v)
            }
        };
        let legs = match (r.media_mode, f.blue) {
            (MediaMode::Redundant, Some(b)) => vec![(0, *f.red.ip(), f.red.port(), f.source), (1, *b.ip(), b.port(), f.source_blue)],
            (MediaMode::Separate, _) if f.port == 2 => vec![(1.min(ports.len() - 1), *f.red.ip(), f.red.port(), f.source)],
            _ => vec![(0, *f.red.ip(), f.red.port(), f.source)],
        };
        // IPMX : les rapports RTCP des flux envoyés, vidéo et son (le JPEG XS sans bloc de média :
        // TR-10-1/2/3 n'en définissent pas pour lui).
        let ipmx_plan = match (ipmx, f.direction, &kind) {
            (Some(refclk), Direction::Output, Kind::Video(_) | Kind::Audio(_)) => {
                let media = match &kind {
                    Kind::Video(v) if f.jpegxs().is_none() => {
                        let (htotal, vtotal, pixel_clock) = super::ipmx::video_totals(v.width, v.height, v.rate, v.interlaced);
                        super::ipmx::MediaInfo::Video(super::ipmx::VideoInfo {
                            sampling: "YCbCr-4:2:2".into(),
                            depth: 10,
                            gpm: false,
                            interlaced: v.interlaced,
                            segmented: false,
                            par: (1, 1),
                            range: "NARROW".into(),
                            colorimetry: "BT709".into(),
                            tcs: "SDR".into(),
                            width: v.width as u16,
                            height: v.height as u16,
                            rate: v.rate,
                            pixel_clock,
                            htotal,
                            vtotal,
                        })
                    }
                    Kind::Audio(a) => super::ipmx::MediaInfo::Audio(super::ipmx::AudioInfo {
                        sample_rate: 48000,
                        bits: a.bits,
                        channels: a.channels.min(255) as u8,
                        ptime_us: a.ptime_us.min(u16::MAX as u32) as u16,
                        measured_rate: 48000,
                        channel_order: match a.channels {
                            1 => "SMPTE2110.(M)".into(),
                            2 => "SMPTE2110.(ST)".into(),
                            n => format!("SMPTE2110.(U{n:02})"),
                        },
                    }),
                    _ => super::ipmx::MediaInfo::None,
                };
                let (packets, octets) = match &kind {
                    Kind::Video(v) => {
                        let lines = if v.interlaced { v.height / 2 } else { v.height } as f64;
                        let bytes = match f.jpegxs() {
                            Some(bpp) => v.width as f64 * lines * bpp as f64 / 8.0,
                            None => v.width as f64 * lines * 2.5,
                        };
                        ((bytes / 1200.0).ceil() as u32, bytes as u32)
                    }
                    Kind::Audio(a) => {
                        let per = 1000 * a.frame_ms / a.ptime_us.max(125);
                        (per, 48 * a.frame_ms * a.channels as u32 * a.bits as u32 / 8)
                    }
                    Kind::Data(_) => (1, 0),
                };
                let ports_ip: Vec<Ipv4Addr> = ports.iter().map(|p| p.2).collect();
                Some(IpmxPlan {
                    ssrc: ssrc_of(&f.id),
                    block: super::ipmx::info_block(refclk, "direct=0", 0, &media),
                    legs: legs
                        .iter()
                        .map(|(port, ip, udp, _)| (ports_ip.get(*port).copied().unwrap_or(Ipv4Addr::UNSPECIFIED), std::net::SocketAddrV4::new(*ip, *udp)))
                        .collect(),
                    tai_offset: cfg.ptp.tai_offset,
                    packets,
                    octets,
                })
            }
            _ => None,
        };
        flows.push(FlowPlan {
            id: f.id.clone(),
            name: f.name.clone(),
            input: f.direction == Direction::Input,
            legs,
            payload_type: f.pt(),
            kind,
            pacing: f.mtl_pacing(),
            jpegxs: f.jpegxs(),
            ipmx: ipmx_plan,
        });
    }
    Ok(Some(Plan { ports, flows, tai_offset: cfg.ptp.tai_offset, bad }))
}

impl Engine {
    /// Tient MTL en accord avec les réglages (bloquant ; rouvre tout quand ils changent).
    /// `ipmx` : l'horloge de référence (`ts-refclk`) quand IPMX est actif.
    pub fn apply(&self, cfg: &HardwareConfig, mtl_loaded: Result<(), String>, mxl: Option<Arc<Mxl>>, ipmx: Option<String>) {
        let planned = plan(cfg, ipmx.as_deref());
        let running_ports = self.running.lock().unwrap().as_ref().map(|r| r.ports.clone());
        // MTL ouvert reste ouvert tant que ST 2110 est en marche (même sans flux : DPDK ne se
        // rouvre pas dans le même processus). Ses ports changés, ou ST 2110 coupé : il est fermé,
        // et ne repartira qu'au redémarrage du Core.
        let ports_change = match (&running_ports, &planned) {
            (Some(rp), Ok(Some(p))) => *rp != (p.ports.clone(), p.tai_offset),
            (Some(_), _) => true,
            (None, _) => false,
        };
        if ports_change {
            let old = self.running.lock().unwrap().take();
            if let Some(r) = old {
                halt(r);
            }
            self.status.lock().unwrap().clear();
        }
        let keep_system = !ports_change && running_ports.is_some() && matches!(&planned, Ok(Some(p)) if p.flows.is_empty());
        // Les hugepages et les ports DPDK ensuite (et rendus quand on n'en veut plus) ; pas pendant
        // que MTL ouvert attend sans flux (il les tient encore).
        let system = if keep_system { Ok(()) } else { super::media::prepare(cfg) };
        let want = match (planned, mtl_loaded, &mxl) {
            (Ok(None), _, _) => None,
            (Err(e), _, _) => {
                self.stop(Some(e));
                return;
            }
            (Ok(Some(_)), Err(e), _) => {
                self.stop(Some(format!("ST 2110 by MTL is not available: {e}")));
                return;
            }
            (Ok(Some(_)), _, None) => {
                self.stop(Some("ST 2110 needs MXL (the flows are shared with the apps through it)".into()));
                return;
            }
            (Ok(Some(p)), Ok(()), Some(_)) => Some(p),
        };
        if let (Some(_), Err(e)) = (&want, &system) {
            self.stop(Some(e.clone()));
            return;
        }
        let mut run = self.running.lock().unwrap();
        let Some(plan) = want else {
            if let Some(r) = run.take() {
                halt(r);
                self.status.lock().unwrap().clear();
            }
            *self.problem.lock().unwrap() = None;
            return;
        };
        // Pas encore ouvert et aucun flux : rien à ouvrir.
        if run.is_none() && plan.flows.is_empty() {
            *self.problem.lock().unwrap() = None;
            return;
        }
        let Some(mxl) = mxl else { return };
        let ports = (plan.ports.clone(), plan.tai_offset);
        if run.is_none() {
            // DPDK s'initialise une fois par processus : déjà ouvert puis fermé, MTL ne repart
            // qu'avec un Core relancé.
            if OPENED.load(Ordering::Relaxed) {
                *self.problem.lock().unwrap() = Some("the media ports changed: restart Strata Core to apply them".into());
                return;
            }
            *self.problem.lock().unwrap() = None;
            let list: Vec<Port> = plan
                .ports
                .iter()
                .map(|(name, pmd, ip, mask)| Port { name: name.clone(), pmd: *pmd, ip: *ip, mask: *mask, gateway: Ipv4Addr::UNSPECIFIED })
                .collect();
            let mtl = match Mtl::open(&list, plan.tai_offset, None) {
                Ok(m) => Arc::new(m),
                Err(e) => {
                    tracing::warn!("MTL: {e}");
                    *self.problem.lock().unwrap() = Some(e);
                    return;
                }
            };
            OPENED.store(true, Ordering::Relaxed);
            tracing::info!("MTL on {}", plan.ports.iter().map(|p| p.0.as_str()).collect::<Vec<_>>().join(", "));
            *run = Some(Running { ports, flows: HashMap::new(), mtl });
        }
        let Some(r) = run.as_mut() else { return };
        // Les flux retirés ou changés s'arrêtent (ensemble, puis on attend leurs fils)…
        // Un flux dont le fil s'est arrêté sur une erreur (lien pas encore monté, file occupée) est
        // relancé, 5 s au plus tôt après son départ.
        let dead = |f: &FlowRun| f.thread.is_finished() && f.started.elapsed() >= Duration::from_secs(5);
        let gone: Vec<String> =
            r.flows.iter().filter(|(id, f)| !plan.flows.iter().any(|p| &p.id == *id && *p == f.plan) || dead(f)).map(|(id, _)| id.clone()).collect();
        let stopping: Vec<FlowRun> = gone.iter().filter_map(|id| r.flows.remove(id)).collect();
        for f in &stopping {
            f.stop.store(true, Ordering::Relaxed);
        }
        for f in stopping {
            let _ = f.thread.join();
            // Un flux relancé garde son état (son erreur reste visible jusqu'à ce qu'il reparte).
            if !plan.flows.iter().any(|p| p.id == f.plan.id) {
                self.status.lock().unwrap().remove(&f.plan.id);
            }
        }
        for (id, e) in &plan.bad {
            let mut st = self.status.lock().unwrap();
            let s = st.entry(id.clone()).or_default();
            s.state = IoState::Error;
            s.error = Some(e.clone());
        }
        // … puis les nouveaux démarrent.
        let new: Vec<&FlowPlan> = plan.flows.iter().filter(|p| !r.flows.contains_key(&p.id)).collect();
        for f in new {
            let stop = Arc::new(AtomicBool::new(false));
            if let Some(thread) = spawn_flow(f, &r.mtl, &mxl, &stop, &self.status) {
                r.flows.insert(f.id.clone(), FlowRun { plan: f.clone(), stop, thread, started: Instant::now() });
            }
        }
    }

    fn stop(&self, problem: Option<String>) {
        if let Some(r) = self.running.lock().unwrap().take() {
            halt(r);
        }
        self.status.lock().unwrap().clear();
        *self.problem.lock().unwrap() = problem;
    }

    pub fn stop_all(&self) {
        self.stop(None);
    }

    /// Le Core tient-il les flux (MTL ouvert) ?
    pub fn active(&self) -> bool {
        match self.running.try_lock() {
            Ok(r) => r.is_some(),
            // Apply en cours : MTL ouvert ou sur le point de l'être.
            Err(_) => true,
        }
    }

    /// Les compteurs de chaque port ouvert par MTL (dans l'ordre : port 1, port 2).
    pub fn port_stats(&self) -> Vec<Option<strata_mtl::PortStats>> {
        // Pendant qu'apply ouvre MTL ou relance des flux (quelques secondes) : rien plutôt
        // qu'attendre.
        let Ok(run) = self.running.try_lock() else { return Vec::new() };
        let Some(r) = run.as_ref() else { return Vec::new() };
        (0..r.ports.0.len()).map(|i| r.mtl.port_stats(i)).collect()
    }

    pub fn status_of(&self, id: &str) -> Option<IoStatus> {
        self.status.lock().unwrap().get(id).cloned()
    }

    pub fn problem(&self) -> Option<String> {
        self.problem.lock().unwrap().clone()
    }
}

/// Le nom d'un son (« 2 can. L24 »).
fn audio_name(a: &strata_mtl::Audio) -> String {
    format!("{} ch · L{}", a.channels, a.bits)
}

/// Son reçu : MTL → flux audio MXL `2110/<flux>`.
fn receive_audio(f: &FlowPlan, a: &strata_mtl::Audio, mtl: &Mtl, mxl: &Mxl, stop: &AtomicBool, set: Setter) -> Result<(), String> {
    // À la réception, tout type de charge est accepté : l'adresse et le port suffisent.
    let rx = mtl.arx(&f.name, &legs(f), 0, a)?;
    let id = strata_mxl::flow_id(&format!("2110/{}", f.id));
    let mut w = mxl.audio_writer(id, &f.name, a.channels as u32)?;
    set(&|s| s.format = Some(audio_name(a)));
    let mut last = Instant::now();
    let mut blocks = 0u64;
    while !stop.load(Ordering::Relaxed) {
        let got = match rx.next() {
            Some(pcm) => {
                w.write_interleaved(&pcm)?;
                blocks += 1;
                true
            }
            None => false,
        };
        if last.elapsed() >= Duration::from_secs(1) || !got {
            let st = rx.stats();
            set(&|s| {
                s.state = if got { IoState::Running } else { IoState::Idle };
                s.frames = blocks;
                s.count(st, f.legs.len());
                s.error = None;
            });
            last = Instant::now();
        }
    }
    Ok(())
}

/// Son envoyé : flux audio MXL `2110/<flux>` (écrit par l'appli) → MTL ; du silence sans lui.
/// MTL cadence l'envoi (un bloc de 10 ms libre à la fois).
fn send_audio(f: &FlowPlan, a: &strata_mtl::Audio, mtl: &Mtl, mxl: &Mxl, stop: &AtomicBool, set: Setter) -> Result<(), String> {
    let tx = mtl.atx(&f.name, &legs(f), f.payload_type, a, f.ipmx.as_ref().map(|p| p.ssrc))?;
    // IPMX : un rapport RTCP par bloc de 10 ms (TR-10-1 § 8.10.1).
    let mut clock = f.ipmx.as_ref().map(|p| IpmxSend::open(p, a.frame_ms as u64 * 1_000_000, 48_000));
    let id = strata_mxl::flow_id(&format!("2110/{}", f.id));
    set(&|s| s.format = Some(audio_name(a)));
    let n = tx.frame_samples();
    let ch = a.channels as usize;
    let mut reader: Option<strata_mxl::AudioReader> = None;
    let (mut blocks, mut heard) = (0u64, 0u64);
    // L'état chaque seconde, comme à la réception : les deux compteurs se comparent.
    let mut tick = Instant::now();
    // L'écrivain de l'appli, revu deux fois par seconde (pas à chaque bloc de 10 ms).
    let mut checked: Option<Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        if checked.is_none_or(|t| t.elapsed() >= Duration::from_millis(500)) {
            checked = Some(Instant::now());
            if mxl.is_active(id) {
                if reader.is_none() {
                    reader = mxl.audio_reader(id).ok();
                }
            } else {
                reader = None;
            }
        }
        // Les canaux de l'appli rangés dans ceux du flux (en trop : ignorés ; en moins : silence).
        let pcm = reader.as_mut().and_then(|r| {
            let rc = r.channels.max(1);
            r.next_interleaved(n).map(|s| if rc == ch { s } else { (0..n * ch).map(|k| if k % ch < rc { s[(k / ch) * rc + k % ch] } else { 0.0 }).collect() })
        });
        if pcm.is_some() {
            heard += 1;
        }
        let pcm = pcm.unwrap_or_else(|| vec![0.0; n * ch]);
        let at = clock.as_mut().map_or(0, IpmxSend::next);
        if tx.send_at(&pcm, at) {
            blocks += 1;
        }
        if tick.elapsed() >= Duration::from_secs(1) {
            tick = Instant::now();
            let on = heard > 0;
            set(&|s| {
                s.state = if on { IoState::Running } else { IoState::Idle };
                s.frames = blocks;
                s.error = clock.as_ref().and_then(|c| c.error.clone());
            });
            heard = 0;
        }
    }
    Ok(())
}

/// La cadence des grains de données MXL : une par trame en entrelacé (comme les grains vidéo).
fn data_rate(v: &strata_mtl::Video) -> (u32, u32) {
    if v.interlaced {
        (v.rate.0 * 2, v.rate.1)
    } else {
        v.rate
    }
}

fn to_mxl(a: strata_mtl::Anc) -> strata_mxl::anc::Packet {
    strata_mxl::anc::Packet { c: a.c, line: a.line, offset: a.offset, s: a.s, stream: a.stream, did: a.did, sdid: a.sdid, udw: a.udw }
}

fn to_mtl(p: strata_mxl::anc::Packet) -> strata_mtl::Anc {
    strata_mtl::Anc { c: p.c, line: p.line, offset: p.offset, s: p.s, stream: p.stream, did: p.did, sdid: p.sdid, udw: p.udw }
}

/// Données reçues : MTL → flux de données MXL `2110/<flux>` (un grain par bloc reçu).
fn receive_data(f: &FlowPlan, v: &strata_mtl::Video, mtl: &Mtl, mxl: &Mxl, stop: &AtomicBool, set: Setter) -> Result<(), String> {
    let rx = mtl.drx(&f.name, &legs(f), 0, v.interlaced)?;
    let id = strata_mxl::flow_id(&format!("2110/{}", f.id));
    let mut w = mxl.data_writer(id, &f.name, data_rate(v))?;
    set(&|s| s.format = Some(format!("ANC · {}", format_name(v))));
    let mut last = Instant::now();
    let mut blocks = 0u64;
    while !stop.load(Ordering::Relaxed) {
        let got = match rx.next() {
            Some((packets, field)) => {
                w.write(&packets.into_iter().map(to_mxl).collect::<Vec<_>>(), field)?;
                blocks += 1;
                true
            }
            None => false,
        };
        if last.elapsed() >= Duration::from_secs(1) || !got {
            let st = rx.stats();
            set(&|s| {
                s.state = if got { IoState::Running } else { IoState::Idle };
                s.frames = blocks;
                s.count(st, f.legs.len());
                s.error = None;
            });
            last = Instant::now();
        }
    }
    Ok(())
}

/// Données envoyées : flux de données MXL `2110/<flux>` (écrit par l'appli) → MTL, un bloc par
/// image (par trame en entrelacé), vide sans elle. MTL cadence l'envoi.
fn send_data(f: &FlowPlan, v: &strata_mtl::Video, mtl: &Mtl, mxl: &Mxl, stop: &AtomicBool, set: Setter) -> Result<(), String> {
    let tx = mtl.dtx(&f.name, &legs(f), f.payload_type, v)?;
    let id = strata_mxl::flow_id(&format!("2110/{}", f.id));
    set(&|s| s.format = Some(format!("ANC · {}", format_name(v))));
    let mut reader: Option<strata_mxl::DataReader> = None;
    let (mut blocks, mut heard) = (0u64, 0u64);
    let mut tick = Instant::now();
    let mut checked: Option<Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        if checked.is_none_or(|t| t.elapsed() >= Duration::from_millis(500)) {
            checked = Some(Instant::now());
            if mxl.is_active(id) {
                if reader.is_none() {
                    reader = mxl.data_reader(id).ok();
                }
            } else {
                reader = None;
            }
        }
        let packets: Vec<strata_mtl::Anc> = match reader.as_mut().and_then(|r| r.next_grain()) {
            Some((p, _)) => {
                heard += 1;
                p.into_iter().map(to_mtl).collect()
            }
            None => Vec::new(),
        };
        if tx.send(&packets) {
            blocks += 1;
        }
        if tick.elapsed() >= Duration::from_secs(1) {
            tick = Instant::now();
            let on = heard > 0;
            set(&|s| {
                s.state = if on { IoState::Running } else { IoState::Idle };
                s.frames = blocks;
                s.error = None;
            });
            heard = 0;
        }
    }
    Ok(())
}

fn halt(r: Running) {
    for f in r.flows.values() {
        f.stop.store(true, Ordering::Relaxed);
    }
    for (_, f) in r.flows {
        let _ = f.thread.join();
    }
}

/// Lance le fil d'un flux.
fn spawn_flow(f: &FlowPlan, mtl: &Arc<Mtl>, mxl: &Arc<Mxl>, stop: &Arc<AtomicBool>, status: &Arc<Mutex<HashMap<String, IoStatus>>>) -> Option<JoinHandle<()>> {
    let (f, mtl, mxl, stop, status) = (f.clone(), mtl.clone(), mxl.clone(), stop.clone(), status.clone());
    let name = format!("2110-{}", f.id);
    std::thread::Builder::new()
        .name(name)
        .spawn(move || {
            let set = |s: &dyn Fn(&mut IoStatus)| s(status.lock().unwrap().entry(f.id.clone()).or_default());
            let r = match (&f.kind, f.input) {
                (Kind::Video(v), true) => receive(&f, v, &mtl, &mxl, &stop, &set),
                (Kind::Video(v), false) => send(&f, v, &mtl, &mxl, &stop, &set),
                (Kind::Audio(a), true) => receive_audio(&f, a, &mtl, &mxl, &stop, &set),
                (Kind::Audio(a), false) => send_audio(&f, a, &mtl, &mxl, &stop, &set),
                (Kind::Data(v), true) => receive_data(&f, v, &mtl, &mxl, &stop, &set),
                (Kind::Data(v), false) => send_data(&f, v, &mtl, &mxl, &stop, &set),
            };
            if let Err(e) = r {
                tracing::warn!("ST 2110 flow {}: {e}", f.name);
                set(&|s| {
                    s.state = IoState::Error;
                    s.error = Some(e.clone());
                });
            }
        })
        .ok()
}

fn format_name(v: &strata_mtl::Video) -> String {
    let r = v.rate.0 as f64 / v.rate.1.max(1) as f64 * if v.interlaced { 2.0 } else { 1.0 };
    let r = (r * 100.0).round() / 100.0;
    format!("{}{}{}", v.height, if v.interlaced { 'i' } else { 'p' }, if r.fract() == 0.0 { format!("{r:.0}") } else { format!("{r}") })
}

fn legs(f: &FlowPlan) -> Vec<Leg> {
    f.legs.iter().map(|&(port, ip, udp, source)| Leg { port, ip, udp, source }).collect()
}

/// Change l'état d'un flux.
type Setter<'a> = &'a dyn Fn(&dyn Fn(&mut IoStatus));

fn mxl_video(v: &strata_mtl::Video) -> strata_mxl::Video {
    strata_mxl::Video { width: v.width, height: v.height, rate: v.rate, interlaced: v.interlaced }
}

/// Reçu : MTL → flux MXL `2110/<flux>`.
fn receive(f: &FlowPlan, video: &strata_mtl::Video, mtl: &Mtl, mxl: &Mxl, stop: &AtomicBool, set: Setter) -> Result<(), String> {
    // JPEG XS : décodé par le greffon, rendu en v210 comme le non compressé.
    let mut rx: Box<dyn strata_mtl::VideoRx> = match f.jpegxs {
        Some(_) => Box::new(mtl.rx22(&f.name, &legs(f), f.payload_type, video)?),
        None => Box::new(mtl.rx(&f.name, &legs(f), f.payload_type, video)?),
    };
    let id = strata_mxl::flow_id(&format!("2110/{}", f.id));
    let mut w = mxl.video_writer(id, &f.name, &mxl_video(video))?;
    set(&|s| s.format = Some(format_name(video)));
    let mut last = Instant::now();
    let mut frames = 0u64;
    while !stop.load(Ordering::Relaxed) {
        let got = match rx.next_frame() {
            Some(fr) => {
                if fr.complete {
                    if video.interlaced {
                        w.write_field_v210(fr.data, fr.linesize, fr.second_field)?;
                    } else {
                        w.write_v210(fr.data, fr.linesize)?;
                    }
                    frames += 1;
                }
                true
            }
            None => false,
        };
        if last.elapsed() >= Duration::from_secs(1) || !got {
            let st = rx.counters();
            set(&|s| {
                s.state = if got { IoState::Running } else { IoState::Idle };
                s.frames = frames;
                s.count(st, f.legs.len());
                s.error = None;
            });
            last = Instant::now();
        }
    }
    Ok(())
}

/// Envoyé : flux MXL `2110/<flux>` (écrit par l'appli) → MTL ; du noir quand rien n'arrive.
fn send(f: &FlowPlan, video: &strata_mtl::Video, mtl: &Mtl, mxl: &Mxl, stop: &AtomicBool, set: Setter) -> Result<(), String> {
    // JPEG XS : l'image v210 est convertie et codée par le greffon en l'envoyant.
    let mut tx: Box<dyn strata_mtl::VideoTx> = match f.jpegxs {
        Some(bpp) => Box::new(mtl.tx22(&f.name, &legs(f), f.payload_type, video, bpp, f.ipmx.as_ref().map(|p| p.ssrc))?),
        None => Box::new(mtl.tx(&f.name, &legs(f), f.payload_type, video, f.pacing, f.ipmx.as_ref().map(|p| p.ssrc))?),
    };
    // IPMX : un rapport RTCP par image, par trame en entrelacé (TR-10-1 § 8.8.2).
    let field_ns = 1_000_000_000u64 * video.rate.1 as u64 / (video.rate.0 as u64 * if video.interlaced { 2 } else { 1 }).max(1);
    let mut clock = f.ipmx.as_ref().map(|p| IpmxSend::open(p, field_ns, 90_000));
    let id = strata_mxl::flow_id(&format!("2110/{}", f.id));
    let v = *video;
    set(&|s| s.format = Some(format_name(&v)));
    let stride = strata_mxl::v210_stride(v.width);
    let black: Vec<u8> = {
        let uyvy: Vec<u8> = (0..v.width as usize * 2).map(|i| if i % 2 == 0 { 128 } else { 16 }).collect();
        let mut l = vec![0u8; stride];
        strata_mxl::uyvy_to_v210(&uyvy, &mut l, v.width as usize);
        l
    };
    let mut follow = strata_mxl::Follow::new(id);
    // En entrelacé, la trame du bas de l'image en cours, gardée pour le paquet suivant (tampon
    // réutilisé).
    let mut bottom: Vec<u8> = Vec::new();
    let mut live = false;
    let mut frames = 0u64;
    let mut second = false;
    let mut tick = Instant::now();
    let period = Duration::from_secs_f64(v.rate.1 as f64 / v.rate.0.max(1) as f64);
    let lines = if v.interlaced { v.height as usize / 2 } else { v.height as usize };
    let black_field = black.repeat(lines);
    // Des lignes de `stride` octets vers le tampon de MTL (une seule copie, ligne à ligne si
    // les pas diffèrent).
    let put = |dst: &mut [u8], ls: usize, src: &[u8]| {
        if ls == stride {
            let n = (lines * stride).min(dst.len()).min(src.len());
            dst[..n].copy_from_slice(&src[..n]);
        } else {
            for y in 0..lines {
                let d = &mut dst[y * ls..y * ls + stride.min(ls)];
                d.copy_from_slice(&src[y * stride..y * stride + d.len()]);
            }
        }
    };
    while !stop.load(Ordering::Relaxed) {
        // Le flux de l'appli : pris s'il a la taille du flux envoyé (la cadence de l'appli peut
        // différer : on envoie sa dernière image à la cadence du flux).
        let mut mismatch = None;
        // Rouvert quand l'appli recrée son flux à un autre format.
        if let Some(r) = follow.reader(mxl).filter(|r| (r.video.width, r.video.height) != (v.width, v.height)) {
            mismatch = Some(format!(
                "the app writes {}, the flow is set to {}",
                format_name(&strata_mtl::Video { width: r.video.width, height: r.video.height, rate: r.video.rate, interlaced: r.video.interlaced }),
                format_name(&v)
            ));
        }
        let Some(mut out) = tx.next_frame() else { continue };
        let ls = out.linesize;
        if second {
            // La trame du bas, gardée avec celle du haut.
            if live {
                put(out.data, ls, &bottom);
            } else {
                put(out.data, ls, &black_field);
            }
        } else {
            // L'image la plus récente, copiée de MXL droit dans le tampon de MTL. Rien de nouveau :
            // la dernière (toujours dans MXL) pendant une seconde, puis du noir.
            let src = follow.reader(mxl).filter(|_| mismatch.is_none());
            // L'image de l'appli (une image, ou ses deux trames) vers ce flux (progressif, ou une
            // trame maintenant et l'autre au paquet suivant) : une copie par ligne, pas plus.
            let mut fill = |a: &[u8], b: Option<&[u8]>| {
                let data = &mut *out.data;
                fn row(src: &[u8], y: usize, stride: usize) -> &[u8] {
                    &src[y * stride..(y + 1) * stride]
                }
                match (v.interlaced, b) {
                    (false, None) => put(data, ls, a),
                    (false, Some(b)) => {
                        for y in 0..lines {
                            let src = if y % 2 == 0 { row(a, y / 2, stride) } else { row(b, y / 2, stride) };
                            data[y * ls..y * ls + stride.min(ls)].copy_from_slice(&src[..stride.min(ls)]);
                        }
                    }
                    (true, Some(b)) => {
                        put(data, ls, a);
                        bottom.clear();
                        bottom.extend_from_slice(b);
                    }
                    (true, None) => {
                        bottom.resize(lines * stride, 0);
                        for y in 0..lines {
                            data[y * ls..y * ls + stride.min(ls)].copy_from_slice(&row(a, 2 * y, stride)[..stride.min(ls)]);
                            bottom[y * stride..(y + 1) * stride].copy_from_slice(row(a, 2 * y + 1, stride));
                        }
                    }
                }
            };
            live = src.and_then(|r| r.with_latest(period * 3, &mut fill)).is_some()
                || src.and_then(|r| r.with_latest(Duration::from_secs(1), &mut fill)).is_some();
            if !live {
                put(out.data, ls, &black_field);
            }
        }
        out.second_field = second;
        if let Some(c) = clock.as_mut() {
            out.timestamp = c.next();
        }
        out.send_field(second);
        frames += 1;
        if v.interlaced {
            second = !second;
        }
        if tick.elapsed() >= Duration::from_secs(1) {
            tick = Instant::now();
            let on = live;
            set(&|s| {
                s.state = if on { IoState::Running } else { IoState::Idle };
                s.frames = frames;
                s.error = mismatch.clone().or_else(|| clock.as_ref().and_then(|c| c.error.clone()));
            });
        }
    }
    Ok(())
}
