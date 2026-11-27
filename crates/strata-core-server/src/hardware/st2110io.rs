//! Les flux ST 2110 vidéo reçus et envoyés par le Core, avec Intel Media Transport Library
//! (MTL) sur les ports média, et MXL pour les applis :
//!
//! - **Reçu** : MTL reçoit le flux (une ou deux pattes, 2022-7) et le Core l'écrit en flux MXL
//!   `2110/<flux>` ; les applis le lisent comme une entrée SDI.
//! - **Envoyé** : l'appli écrit le flux MXL `2110/<flux>` ; le Core l'envoie avec MTL (cadencé
//!   ST 2110-21) ; du noir quand rien n'arrive.
//! - Le mode des ports (Réglages › Réseau) : socket du noyau, AF_XDP ou DPDK. « Off » : les flux
//!   sont seulement réglés (et regardés dans l'aperçu), comme avant.
//! - Tout est rouvert quand les réglages des ports ou des flux changent.
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
use strata_mxl::Mxl;
use strata_mtl::{Leg, Mtl, Pmd, Port};

use super::{net, parse_format, Direction, Essence, HardwareConfig, MediaIo, MediaMode};

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
    /// Reçu : paquets reçus et perdus (après 2022-7).
    pub packets: u64,
    pub lost: u64,
    pub error: Option<String>,
}

/// Ce qui fait ouvrir MTL : les ports et les flux. Un changement rouvre tout.
#[derive(Debug, Clone, PartialEq)]
struct Plan {
    ports: Vec<(String, Pmd, Ipv4Addr, Ipv4Addr)>,
    flows: Vec<FlowPlan>,
    tai_offset: u32,
}

#[derive(Debug, Clone, PartialEq)]
struct FlowPlan {
    id: String,
    name: String,
    input: bool,
    legs: Vec<(usize, Ipv4Addr, u16)>,
    payload_type: u8,
    video: strata_mtl::Video,
}

struct Running {
    plan: Plan,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    // Lâché après les fils : ses sessions sont fermées par eux.
    _mtl: Arc<Mtl>,
}

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

/// Ce qu'il faut ouvrir d'après les réglages ; None : rien (mode « Off », aucun flux vidéo).
fn plan(cfg: &HardwareConfig) -> Result<Option<Plan>, String> {
    let r = &cfg.network;
    if r.media_io == MediaIo::Off {
        return Ok(None);
    }
    let video: Vec<_> = cfg.flows.iter().filter(|f| f.essence == Essence::Video).collect();
    if video.is_empty() {
        return Ok(None);
    }
    let two = r.media_mode == MediaMode::Redundant && video.iter().any(|f| f.blue.is_some()) || video.iter().any(|f| f.port == 2);
    let mut ports = vec![port_of(r.media_io, &r.media1, &r.media1_address)?];
    if two {
        ports.push(port_of(r.media_io, &r.media2, &r.media2_address)?);
    }
    let house = {
        let (n, d) = cfg.clock.house.rate.ratio();
        let interlaced = matches!((n, d), (25, 1) | (30000, 1001));
        (1920, 1080, (n as u32, d as u32), interlaced)
    };
    let mut flows = Vec::new();
    for f in video {
        let (width, height, rate, interlaced) = if f.format.trim().is_empty() { house } else { parse_format(&f.format).ok_or(format!("{}: unknown format {}", f.name, f.format))? };
        let legs = match (r.media_mode, f.blue) {
            (MediaMode::Redundant, Some(b)) => vec![(0, *f.red.ip(), f.red.port()), (1, *b.ip(), b.port())],
            (MediaMode::Separate, _) if f.port == 2 => vec![(1.min(ports.len() - 1), *f.red.ip(), f.red.port())],
            _ => vec![(0, *f.red.ip(), f.red.port())],
        };
        flows.push(FlowPlan {
            id: f.id.clone(),
            name: f.name.clone(),
            input: f.direction == Direction::Input,
            legs,
            payload_type: f.payload_type,
            video: strata_mtl::Video { width, height, rate, interlaced },
        });
    }
    Ok(Some(Plan { ports, flows, tai_offset: cfg.ptp.tai_offset }))
}

impl Engine {
    /// Tient MTL en accord avec les réglages (bloquant ; rouvre tout quand ils changent).
    pub fn apply(&self, cfg: &HardwareConfig, mtl_loaded: Result<(), String>, mxl: Option<Arc<Mxl>>) {
        // Les hugepages et les ports DPDK d'abord (et rendus quand on n'en veut plus).
        let system = super::media::prepare(cfg);
        let want = match (plan(cfg), mtl_loaded, &mxl) {
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
        if run.as_ref().map(|r| &r.plan) == want.as_ref() {
            return;
        }
        if let Some(r) = run.take() {
            halt(r);
        }
        self.status.lock().unwrap().clear();
        *self.problem.lock().unwrap() = None;
        let (Some(plan), Some(mxl)) = (want, mxl) else { return };
        let ports: Vec<Port> = plan
            .ports
            .iter()
            .map(|(name, pmd, ip, mask)| Port { name: name.clone(), pmd: *pmd, ip: *ip, mask: *mask, gateway: Ipv4Addr::UNSPECIFIED })
            .collect();
        let mtl = match Mtl::open(&ports, plan.tai_offset, None) {
            Ok(m) => Arc::new(m),
            Err(e) => {
                tracing::warn!("MTL: {e}");
                *self.problem.lock().unwrap() = Some(e);
                return;
            }
        };
        tracing::info!("MTL on {} ({} flows)", plan.ports.iter().map(|p| p.0.as_str()).collect::<Vec<_>>().join(", "), plan.flows.len());
        let stop = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::new();
        for f in &plan.flows {
            let (f, mtl, mxl, stop, status) = (f.clone(), mtl.clone(), mxl.clone(), stop.clone(), self.status.clone());
            let name = format!("2110-{}", f.id);
            if let Ok(t) = std::thread::Builder::new().name(name).spawn(move || {
                let set = |s: &dyn Fn(&mut IoStatus)| s(status.lock().unwrap().entry(f.id.clone()).or_default());
                let r = if f.input { receive(&f, &mtl, &mxl, &stop, &set) } else { send(&f, &mtl, &mxl, &stop, &set) };
                if let Err(e) = r {
                    tracing::warn!("ST 2110 flow {}: {e}", f.name);
                    set(&|s| {
                        s.state = IoState::Error;
                        s.error = Some(e.clone());
                    });
                }
            }) {
                threads.push(t);
            }
        }
        *run = Some(Running { plan, stop, threads, _mtl: mtl });
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
        self.running.lock().unwrap().is_some()
    }

    pub fn status_of(&self, id: &str) -> Option<IoStatus> {
        self.status.lock().unwrap().get(id).cloned()
    }

    pub fn problem(&self) -> Option<String> {
        self.problem.lock().unwrap().clone()
    }
}

fn halt(r: Running) {
    r.stop.store(true, Ordering::Relaxed);
    for t in r.threads {
        let _ = t.join();
    }
}

fn format_name(v: &strata_mtl::Video) -> String {
    let r = v.rate.0 as f64 / v.rate.1.max(1) as f64 * if v.interlaced { 2.0 } else { 1.0 };
    let r = (r * 100.0).round() / 100.0;
    format!("{}{}{}", v.height, if v.interlaced { 'i' } else { 'p' }, if r.fract() == 0.0 { format!("{r:.0}") } else { format!("{r}") })
}

fn legs(f: &FlowPlan) -> Vec<Leg> {
    f.legs.iter().map(|(port, ip, udp)| Leg { port: *port, ip: *ip, udp: *udp }).collect()
}

/// Change l'état d'un flux.
type Setter<'a> = &'a dyn Fn(&dyn Fn(&mut IoStatus));

fn mxl_video(v: &strata_mtl::Video) -> strata_mxl::Video {
    strata_mxl::Video { width: v.width, height: v.height, rate: v.rate, interlaced: v.interlaced }
}

/// Reçu : MTL → flux MXL `2110/<flux>`.
fn receive(f: &FlowPlan, mtl: &Mtl, mxl: &Mxl, stop: &AtomicBool, set: Setter) -> Result<(), String> {
    let rx = mtl.rx(&f.name, &legs(f), f.payload_type, &f.video)?;
    let id = strata_mxl::flow_id(&format!("2110/{}", f.id));
    let mut w = mxl.video_writer(id, &f.name, &mxl_video(&f.video))?;
    set(&|s| s.format = Some(format_name(&f.video)));
    let mut last = Instant::now();
    let mut frames = 0u64;
    while !stop.load(Ordering::Relaxed) {
        let got = match rx.next() {
            Some(fr) => {
                if fr.complete {
                    if f.video.interlaced {
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
            let (packets, lost) = rx.stats();
            set(&|s| {
                s.state = if got { IoState::Running } else { IoState::Idle };
                s.frames = frames;
                s.packets = packets;
                s.lost = lost;
                s.error = None;
            });
            last = Instant::now();
        }
    }
    Ok(())
}

/// Envoyé : flux MXL `2110/<flux>` (écrit par l'appli) → MTL ; du noir quand rien n'arrive.
fn send(f: &FlowPlan, mtl: &Mtl, mxl: &Mxl, stop: &AtomicBool, set: Setter) -> Result<(), String> {
    let tx = mtl.tx(&f.name, &legs(f), f.payload_type, &f.video)?;
    let id = strata_mxl::flow_id(&format!("2110/{}", f.id));
    let v = f.video;
    set(&|s| s.format = Some(format_name(&v)));
    let stride = strata_mxl::v210_stride(v.width);
    let black: Vec<u8> = {
        let uyvy: Vec<u8> = (0..v.width as usize * 2).map(|i| if i % 2 == 0 { 128 } else { 16 }).collect();
        let mut l = vec![0u8; stride];
        strata_mxl::uyvy_to_v210(&uyvy, &mut l, v.width as usize);
        l
    };
    let mut reader: Option<strata_mxl::VideoReader> = None;
    let mut frame: Option<Vec<u8>> = None;
    let mut fresh = Instant::now() - Duration::from_secs(10);
    let mut frames = 0u64;
    let mut second = false;
    let period = Duration::from_secs_f64(v.rate.1 as f64 / v.rate.0.max(1) as f64);
    while !stop.load(Ordering::Relaxed) {
        if mxl.is_active(id) {
            if reader.as_ref().is_none_or(|r| r.video != mxl_video(&v)) {
                reader = mxl.video_reader(id).ok().filter(|r| r.video == mxl_video(&v));
            }
        } else {
            reader = None;
        }
        // Une nouvelle image par image du flux (en entrelacé : avant sa première trame).
        if !second {
            if let Some(img) = reader.as_ref().and_then(|r| r.latest(period * 3)) {
                frame = Some(img);
                fresh = Instant::now();
            } else if fresh.elapsed() > Duration::from_secs(1) {
                frame = None;
            }
        }
        let Some(mut out) = tx.next() else { continue };
        let rows = out.height as usize;
        let ls = out.linesize;
        for y in 0..rows {
            let src_row = if v.interlaced { 2 * y + second as usize } else { y };
            let dst = &mut out.data[y * ls..y * ls + stride.min(ls)];
            match &frame {
                Some(img) => dst.copy_from_slice(&img[src_row * stride..src_row * stride + dst.len()]),
                None => dst.copy_from_slice(&black[..dst.len()]),
            }
        }
        out.second_field = second;
        out.send_field(second);
        frames += 1;
        if v.interlaced {
            second = !second;
        }
        if frames.is_multiple_of(25) {
            let on = frame.is_some();
            set(&|s| {
                s.state = if on { IoState::Running } else { IoState::Idle };
                s.frames = frames;
                s.error = None;
            });
        }
    }
    Ok(())
}
