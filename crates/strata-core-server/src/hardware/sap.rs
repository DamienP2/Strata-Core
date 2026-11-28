//! SAP (RFC 2974) : les flux de son AES67 annoncés sur le réseau (Dante en mode AES67, consoles,
//! passerelles), et nos flux de son envoyés annoncés à notre tour pour que Dante Controller les voie.
//!
//! Une annonce part toutes les 30 s vers 239.255.255.255:9875 ; elle porte le SDP du flux. Le Core
//! écoute sur ses ports média et de gestion, garde ce qu'il entend (5 min sans nouvelle : oublié),
//! et ne reçoit rien de lui-même : la page propose, l'administrateur choisit.
use serde::Serialize;
use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use super::{Direction, Essence, HardwareConfig};
use crate::nmos::sdp;

pub const GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 255);
pub const PORT: u16 = 9875;
/// Une annonce toutes les 30 s ; oubliée après 5 min sans nouvelle.
const EVERY: Duration = Duration::from_secs(30);
const FORGET: Duration = Duration::from_secs(300);
const SDP_TYPE: &[u8] = b"application/sdp\0";

/// Un paquet SAP : retrait ou annonce, son identifiant, l'adresse d'origine, le SDP.
#[derive(Debug, Clone, PartialEq)]
pub struct Packet {
    pub delete: bool,
    pub hash: u16,
    pub origin: Ipv4Addr,
    pub sdp: String,
}

pub fn encode(p: &Packet) -> Vec<u8> {
    let mut b = vec![0x20 | if p.delete { 0x04 } else { 0 }, 0];
    b.extend(p.hash.to_be_bytes());
    b.extend(p.origin.octets());
    b.extend(SDP_TYPE);
    b.extend(p.sdp.as_bytes());
    b
}

pub fn decode(b: &[u8]) -> Option<Packet> {
    let head: &[u8; 8] = b.get(..8)?.try_into().ok()?;
    let (flags, auth) = (head[0], head[1] as usize);
    // Version 1, adresses IPv4, ni chiffré ni compressé.
    if flags >> 5 != 1 || flags & 0x10 != 0 || flags & 0x03 != 0 {
        return None;
    }
    let hash = u16::from_be_bytes([head[2], head[3]]);
    let origin = Ipv4Addr::new(head[4], head[5], head[6], head[7]);
    let mut rest = b.get(8 + auth * 4..)?;
    if !rest.starts_with(b"v=0") {
        let end = rest.iter().position(|&c| c == 0)?;
        if &rest[..end] != b"application/sdp" {
            return None;
        }
        rest = &rest[end + 1..];
    }
    Some(Packet { delete: flags & 0x04 != 0, hash, origin, sdp: String::from_utf8_lossy(rest).into_owned() })
}

/// L'identifiant d'une annonce : stable tant que le SDP ne change pas.
fn hash_of(text: &str) -> u16 {
    let mut h: u32 = 2166136261;
    for b in text.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    (h ^ (h >> 16)) as u16
}

/// Un flux annoncé par un autre appareil.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Heard {
    pub name: String,
    pub origin: String,
    pub group: String,
    pub port: u16,
    pub source: Option<String>,
    pub channels: Option<u16>,
    pub bits: Option<u8>,
    pub ptime_us: Option<u32>,
    /// `a=ts-refclk` : l'horloge que suit l'émetteur (à comparer à la nôtre).
    pub clock: Option<String>,
    pub sdp: String,
    pub seen_secs: u64,
}

struct Entry {
    sdp: String,
    seen: Instant,
}

#[derive(Default)]
pub struct Sap {
    heard: Mutex<HashMap<(Ipv4Addr, u16), Entry>>,
    run: Mutex<Option<Run>>,
    /// Nos flux annoncés (leur nom), et pourquoi rien n'écoute.
    pub(crate) announced: Mutex<Vec<String>>,
    error: Mutex<Option<String>>,
}

#[derive(Clone, PartialEq)]
struct Want {
    /// Les adresses des interfaces où écouter et annoncer.
    ifaces: Vec<Ipv4Addr>,
}

struct Run {
    want: Want,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Sap {
    fn note(&self, p: Packet, mine: &[Ipv4Addr]) {
        if mine.contains(&p.origin) {
            return;
        }
        let mut h = self.heard.lock().unwrap_or_else(|e| e.into_inner());
        if p.delete {
            h.remove(&(p.origin, p.hash));
        } else {
            h.insert((p.origin, p.hash), Entry { sdp: p.sdp, seen: Instant::now() });
        }
        h.retain(|_, e| e.seen.elapsed() < FORGET);
    }

    /// Ce qui est entendu, le plus récent d'abord.
    pub fn heard(&self) -> Vec<Heard> {
        let mut h = self.heard.lock().unwrap_or_else(|e| e.into_inner());
        h.retain(|_, e| e.seen.elapsed() < FORGET);
        let mut out: Vec<Heard> = h
            .iter()
            .map(|((origin, _), e)| {
                let line = |k: &str| e.sdp.lines().find_map(|l| l.trim().strip_prefix(k)).map(|v| v.trim().to_string());
                let parsed = sdp::parse(&e.sdp).ok();
                let leg = parsed.as_ref().and_then(|p| p.legs.first().cloned());
                let (channels, bits, ptime_us) = match parsed.as_ref().map(|p| &p.media) {
                    Some(sdp::Media::Audio { channels, bits, ptime_us }) => (Some(*channels), Some(*bits), Some(*ptime_us)),
                    _ => (None, None, None),
                };
                Heard {
                    name: line("s=").unwrap_or_default(),
                    origin: origin.to_string(),
                    group: leg.as_ref().map(|l| l.group.to_string()).unwrap_or_default(),
                    port: leg.as_ref().map(|l| l.port).unwrap_or(0),
                    source: leg.and_then(|l| l.source.map(|s| s.to_string())),
                    channels,
                    bits,
                    ptime_us,
                    clock: line("a=ts-refclk:"),
                    sdp: e.sdp.clone(),
                    seen_secs: e.seen.elapsed().as_secs(),
                }
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name).then(a.origin.cmp(&b.origin)));
        out
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn listening(&self) -> Vec<String> {
        self.run.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|r| r.want.ifaces.iter().map(|i| i.to_string()).collect()).unwrap_or_default()
    }
}

/// Les interfaces où écouter : les ports média (s'ils restent au noyau) et celui de gestion.
fn ifaces_of(cfg: &HardwareConfig) -> Vec<Ipv4Addr> {
    let mut v: Vec<Ipv4Addr> = Vec::new();
    for ip in cfg.media_ips().into_iter().flatten() {
        if cfg.network.media_io != super::MediaIo::Dpdk && !v.contains(&ip) {
            v.push(ip);
        }
    }
    if let Some(ip) = (!cfg.network.management.is_empty()).then(|| super::net::interface_ip(&cfg.network.management)).flatten() {
        if !v.contains(&ip) {
            v.push(ip);
        }
    }
    v
}

/// Tient l'écoute (et l'annonce) en accord avec les réglages (bloquant, rapide).
pub fn apply(s: &crate::state::Shared) {
    let cfg = HardwareConfig::load(&s.data_dir);
    // Dante coupé (Réglages › Fonctions) : ni écoute ni annonce.
    let want = Want { ifaces: if s.features.on(crate::features::DANTE) { ifaces_of(&cfg) } else { Vec::new() } };
    let mut run = s.sap.run.lock().unwrap_or_else(|e| e.into_inner());
    if run.as_ref().is_some_and(|r| r.want == want) {
        return;
    }
    if let Some(r) = run.take() {
        r.stop.store(true, Ordering::Relaxed);
        for t in r.threads {
            let _ = t.join();
        }
    }
    if want.ifaces.is_empty() {
        *s.sap.error.lock().unwrap() =
            s.features.on(crate::features::DANTE).then(|| "no network interface to listen on (management or media port)".to_string());
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let socket = match shared_socket(PORT) {
        Ok(sock) => sock,
        Err(e) => {
            *s.sap.error.lock().unwrap() = Some(format!("cannot listen on UDP port {PORT}: {e}"));
            return;
        }
    };
    let mut joined = Vec::new();
    for ip in &want.ifaces {
        if socket.join_multicast_v4(&GROUP, ip).is_ok() {
            joined.push(*ip);
        }
    }
    let _ = socket.set_read_timeout(Some(Duration::from_secs(1)));
    *s.sap.error.lock().unwrap() = if joined.is_empty() { Some("cannot join the SAP group on any interface".into()) } else { None };
    let mut threads = Vec::new();
    // L'écoute.
    {
        let (stop, state, mine) = (stop.clone(), s.clone(), want.ifaces.clone());
        threads.push(
            std::thread::Builder::new()
                .name("sap-listen".into())
                .spawn(move || {
                    let mut buf = vec![0u8; 4096];
                    while !stop.load(Ordering::Relaxed) {
                        if let Ok((n, _)) = socket.recv_from(&mut buf) {
                            if let Some(p) = decode(&buf[..n]) {
                                state.sap.note(p, &mine);
                            }
                        }
                    }
                })
                .expect("thread"),
        );
    }
    // L'annonce de nos flux de son envoyés (si demandée), toutes les 30 s, retirés à l'arrêt.
    {
        let (stop, state, ifaces) = (stop.clone(), s.clone(), want.ifaces.clone());
        threads.push(std::thread::Builder::new().name("sap-announce".into()).spawn(move || announce_loop(&state, &ifaces, &stop)).expect("thread"));
    }
    tracing::info!("SAP: listening on {}", want.ifaces.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", "));
    *run = Some(Run { want, stop, threads });
}

pub fn stop(s: &crate::state::Shared) {
    if let Some(r) = s.sap.run.lock().unwrap_or_else(|e| e.into_inner()).take() {
        r.stop.store(true, Ordering::Relaxed);
        for t in r.threads {
            let _ = t.join();
        }
    }
}

/// Le port SAP, partagé (SO_REUSEADDR) : d'autres logiciels du serveur écoutent les annonces aussi.
fn shared_socket(port: u16) -> std::io::Result<UdpSocket> {
    use std::os::fd::FromRawFd;
    // SAFETY : un socket créé ici, réglé puis confié à UdpSocket (qui le fermera).
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let sock = UdpSocket::from_raw_fd(fd);
        let one: libc::c_int = 1;
        for opt in [libc::SO_REUSEADDR, libc::SO_REUSEPORT] {
            libc::setsockopt(fd, libc::SOL_SOCKET, opt, &one as *const _ as *const libc::c_void, std::mem::size_of::<libc::c_int>() as u32);
        }
        let addr = libc::sockaddr_in { sin_family: libc::AF_INET as u16, sin_port: port.to_be(), sin_addr: libc::in_addr { s_addr: 0 }, sin_zero: [0; 8] };
        if libc::bind(fd, &addr as *const _ as *const libc::sockaddr, std::mem::size_of::<libc::sockaddr_in>() as u32) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(sock)
    }
}

/// Un socket d'envoi vers le groupe SAP, par l'interface `ip`.
fn sender(ip: Ipv4Addr) -> Option<UdpSocket> {
    let sock = UdpSocket::bind(SocketAddrV4::new(ip, 0)).ok()?;
    let _ = sock.set_multicast_ttl_v4(32);
    // L'interface de sortie du multicast : celle de l'adresse.
    let addr = libc::in_addr { s_addr: u32::from_ne_bytes(ip.octets()) };
    // SAFETY : un setsockopt simple sur un socket à nous, avec la bonne taille.
    unsafe {
        use std::os::fd::AsRawFd;
        libc::setsockopt(
            sock.as_raw_fd(),
            libc::IPPROTO_IP,
            libc::IP_MULTICAST_IF,
            &addr as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::in_addr>() as u32,
        );
    }
    Some(sock)
}

/// Les SDP de nos flux de son envoyés : (nom, SDP, adresse d'origine).
fn our_sdps(s: &crate::state::AppState, cfg: &HardwareConfig) -> Vec<(String, String, Ipv4Addr)> {
    if !cfg.sap.announce {
        return Vec::new();
    }
    let ips = cfg.media_ips();
    cfg.flows
        .iter()
        .filter(|f| f.enabled && f.essence == Essence::Audio && f.direction == Direction::Output)
        .filter_map(|f| {
            let src = ips[(f.port.max(1) - 1).min(1) as usize].or(ips[0])?;
            let mut legs = vec![(*f.red.ip(), f.red.port(), src)];
            if let (Some(b), Some(src2)) = (f.blue, ips[1]) {
                if cfg.network.media_mode == super::MediaMode::Redundant {
                    legs.push((*b.ip(), b.port(), src2));
                }
            }
            let iface = if f.port == 2 { cfg.network.media2.clone() } else { cfg.network.media1.clone() };
            let session = hash_of(&f.id) as u64;
            let (name, _) = cfg.nmos_names(f);
            let text = sdp::make(&sdp::SenderSdp {
                name: &name,
                session,
                version: 1,
                legs,
                media: sdp::Media::Audio { channels: f.channels, bits: f.bits, ptime_us: f.ptime_us },
                payload_type: f.pt(),
                refclk: crate::nmos::refclk(s, cfg, &iface),
                tp: "2110TPN",
                ipmx: false,
            });
            Some((f.name.clone(), text, src))
        })
        .collect()
}

fn announce_loop(s: &crate::state::AppState, ifaces: &[Ipv4Addr], stop: &AtomicBool) {
    let socks: Vec<(Ipv4Addr, UdpSocket)> = ifaces.iter().filter_map(|ip| sender(*ip).map(|sk| (*ip, sk))).collect();
    let dest = SocketAddrV4::new(GROUP, PORT);
    let send = |p: &Packet| {
        let b = encode(p);
        for (_, sk) in &socks {
            let _ = sk.send_to(&b, dest);
        }
    };
    let mut last: Vec<Packet> = Vec::new();
    let mut next = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if Instant::now() >= next {
            next = Instant::now() + EVERY;
            let cfg = HardwareConfig::load(&s.data_dir);
            let now: Vec<(String, Packet)> =
                our_sdps(s, &cfg).into_iter().map(|(name, sdp, origin)| (name, Packet { delete: false, hash: hash_of(&sdp), origin, sdp })).collect();
            // Ce qui n'est plus annoncé : retiré.
            for old in last.iter().filter(|o| !now.iter().any(|(_, p)| p.hash == o.hash && p.origin == o.origin)) {
                send(&Packet { delete: true, ..old.clone() });
            }
            for (_, p) in &now {
                send(p);
            }
            *s.sap.announced.lock().unwrap_or_else(|e| e.into_inner()) = now.iter().map(|(n, _)| n.clone()).collect();
            last = now.into_iter().map(|(_, p)| p).collect();
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    for p in &last {
        send(&Packet { delete: true, ..p.clone() });
    }
    s.sap.announced.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_packet() {
        let p = Packet { delete: false, hash: 0x1234, origin: Ipv4Addr::new(10, 0, 0, 5), sdp: "v=0\r\ns=Stage\r\n".into() };
        let b = encode(&p);
        assert_eq!(&b[..8], &[0x20, 0, 0x12, 0x34, 10, 0, 0, 5]);
        assert_eq!(decode(&b), Some(p.clone()));
        let d = Packet { delete: true, ..p.clone() };
        assert!(decode(&encode(&d)).unwrap().delete);
        // Sans type (permis par la RFC), et avec authentification à sauter.
        let mut bare = vec![0x20, 1, 0, 1, 10, 0, 0, 5, 9, 9, 9, 9];
        bare.extend(b"v=0\r\n");
        assert_eq!(decode(&bare).unwrap().sdp, "v=0\r\n");
        // Chiffré, compressé ou IPv6 : ignoré.
        assert!(decode(&[0x22, 0, 0, 0, 0, 0, 0, 0]).is_none());
        // Un paquet trop court (n'importe qui peut en envoyer) : ignoré, sans paniquer.
        for n in 0..8 {
            assert!(decode(&[0x20, 0, 0, 0, 0, 0, 0, 0][..n]).is_none());
        }
        assert!(decode(&[0x30, 0, 0, 0, 0, 0, 0, 0]).is_none());
    }

    #[test]
    fn keeps_what_others_announce() {
        let sap = Sap::default();
        let sdp = "v=0\r\no=- 1 1 IN IP4 10.0.0.9\r\ns=Dante Stage 1-8\r\nt=0 0\r\nm=audio 5004 RTP/AVP 97\r\nc=IN IP4 239.69.1.2/32\r\na=rtpmap:97 L24/48000/8\r\na=ptime:1\r\na=ts-refclk:ptp=IEEE1588-2008:00-1D-C1-FF-FE-00-00-01:0\r\n";
        let mine = [Ipv4Addr::new(10, 0, 0, 2)];
        sap.note(Packet { delete: false, hash: 7, origin: Ipv4Addr::new(10, 0, 0, 9), sdp: sdp.into() }, &mine);
        // Nos propres annonces : ignorées.
        sap.note(Packet { delete: false, hash: 8, origin: Ipv4Addr::new(10, 0, 0, 2), sdp: sdp.into() }, &mine);
        let h = sap.heard();
        assert_eq!(h.len(), 1);
        assert_eq!(
            (h[0].name.as_str(), h[0].group.as_str(), h[0].port, h[0].channels, h[0].bits, h[0].ptime_us),
            ("Dante Stage 1-8", "239.69.1.2", 5004, Some(8), Some(24), Some(1000))
        );
        assert!(h[0].clock.as_deref().unwrap().ends_with(":0"));
        sap.note(Packet { delete: true, hash: 7, origin: Ipv4Addr::new(10, 0, 0, 9), sdp: String::new() }, &mine);
        assert!(sap.heard().is_empty());
    }
}
