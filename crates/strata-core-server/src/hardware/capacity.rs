//! L'évaluation en temps réel des ports média ST 2110 (Réglages › Matériel › Réseau) :
//!
//! - **Mesuré** : le débit reçu et envoyé par chaque port, d'après ses compteurs (le noyau, ou MTL
//!   en DPDK, où le noyau ne voit plus le port), et les paquets jetés par la carte.
//! - **Prévu** : ce que demandent les flux réglés et actifs (vidéo 4:2:2 10 bits, son PCM, en-têtes
//!   RTP/UDP/IP/Ethernet compris). En 2022-7, chaque flux passe par les deux ports.
//! - **Restant** : ce que le lien laisse (90 % de sa vitesse, la marge d'usage), dans chaque sens
//!   (le lien est full duplex), et combien de flux de chaque format y tiennent encore. En 2022-7,
//!   le port le plus chargé décide ; avec deux ports indépendants, leurs places s'ajoutent.
//!
//! Les alertes (`alerts.rs`) s'en servent : port au-delà de 90 %, lien tombé, réglages qui
//! demandent plus que le lien.
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use super::{net, parse_format, Direction, Essence, Flow, HardwareConfig, MediaIo, MediaMode};
use crate::state::AppState;

/// La part du lien que les flux peuvent prendre.
pub const HEADROOM: f64 = 0.9;
/// En-têtes par paquet vidéo sur le fil : RTP 12 + 2110-20 8 + UDP 8 + IP 20 + Ethernet 14 + FCS 4
/// + préambule 8 + intervalle 12.
const VIDEO_OVERHEAD: f64 = 86.0;
const AUDIO_OVERHEAD: f64 = 78.0;
/// La charge utile d'un paquet vidéo (GPM, 1200 octets : sous la MTU de 1500).
const VIDEO_PAYLOAD: f64 = 1200.0;

/// Le débit sur le fil d'une vidéo 4:2:2 10 bits, en Mb/s.
pub fn video_mbps(w: u32, h: u32, (n, d): (u32, u32)) -> f64 {
    let frame = w as f64 * h as f64 * 2.5;
    let fps = n as f64 / d.max(1) as f64;
    let packets = (frame / VIDEO_PAYLOAD).ceil();
    (frame + packets * VIDEO_OVERHEAD) * fps * 8.0 / 1e6
}

/// Le débit sur le fil d'un son PCM à 48 kHz, en Mb/s.
pub fn audio_mbps(channels: u16, bits: u8, ptime_us: u32) -> f64 {
    let ptime = ptime_us.max(125) as f64;
    let samples = 48.0 * ptime / 1000.0;
    let payload = samples * channels as f64 * (bits as f64 / 8.0);
    (payload + AUDIO_OVERHEAD) * (1e6 / ptime) * 8.0 / 1e6
}

/// Ce que demande un flux, en Mb/s.
pub fn flow_mbps(cfg: &HardwareConfig, f: &Flow) -> f64 {
    match f.essence {
        Essence::Video => cfg.video_of(f).map_or(0.0, |(w, h, r, _)| video_mbps(w, h, r)),
        Essence::Audio => audio_mbps(f.channels, f.bits, f.ptime_us),
        // ST 2110-40 : quelques centaines de kb/s.
        Essence::Data => 0.5,
    }
}

#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct Both {
    pub rx: f64,
    pub tx: f64,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct PortLoad {
    /// 1 ou 2.
    pub n: u8,
    pub iface: String,
    /// Vitesse du lien, Mb/s (None : inconnue, lien tombé ou port en DPDK jamais vu).
    pub speed: Option<u32>,
    pub up: Option<bool>,
    /// Mesuré, Mb/s (None : pas encore deux relevés, ou compteurs absents).
    pub measured: Option<Both>,
    /// Paquets jetés par la carte, par seconde.
    pub dropped: Option<f64>,
    /// Ce que demandent les flux réglés.
    pub planned: Both,
    /// Le plus grand du mesuré et du prévu.
    pub used: Both,
    /// Ce qui reste sous la marge (None : vitesse inconnue).
    pub free: Option<Both>,
    /// Le plus chargé des deux sens, en % de la vitesse du lien.
    pub percent: Option<f64>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FormatRoom {
    pub name: String,
    pub mbps: f64,
    /// Flux de ce format qui tiennent encore, reçus et envoyés.
    pub rx: Option<u32>,
    pub tx: Option<u32>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Capacity {
    pub redundant: bool,
    /// Le format maison (en 1080), pour le résumé « encore N flux ».
    pub house: Option<String>,
    pub headroom: f64,
    pub ports: Vec<PortLoad>,
    pub formats: Vec<FormatRoom>,
}

/// Les relevés précédents, pour en tirer un débit.
#[derive(Default)]
pub struct Meter {
    /// Par interface : l'heure, les octets reçus, envoyés, les paquets jetés ; le dernier débit.
    last: Mutex<HashMap<String, Sample>>,
    /// La dernière vitesse vue de chaque interface (en DPDK, le noyau ne la donne plus).
    speeds: Mutex<HashMap<String, u32>>,
}

struct Sample {
    at: Instant,
    rx: u64,
    tx: u64,
    dropped: u64,
    rate: Option<(Both, f64)>,
}

fn sysfs(iface: &str, what: &str) -> Option<u64> {
    std::fs::read_to_string(format!("/sys/class/net/{iface}/statistics/{what}")).ok()?.trim().parse().ok()
}

impl Meter {
    /// Le débit depuis le relevé précédent (au moins 0,5 s avant ; sinon le dernier calculé).
    fn rate(&self, iface: &str, counters: Option<(u64, u64, u64)>) -> Option<(Both, f64)> {
        let (rx, tx, dropped) = counters?;
        let now = Instant::now();
        let mut last = self.last.lock().unwrap();
        let e = last.entry(iface.to_string()).or_insert(Sample { at: now, rx, tx, dropped, rate: None });
        let dt = now.duration_since(e.at);
        if dt < Duration::from_millis(500) {
            return e.rate;
        }
        // Compteurs remis à zéro (MTL rouvert) : on repart.
        let rate = (rx >= e.rx && tx >= e.tx).then(|| {
            let s = dt.as_secs_f64();
            (Both { rx: (rx - e.rx) as f64 * 8.0 / s / 1e6, tx: (tx - e.tx) as f64 * 8.0 / s / 1e6 }, dropped.saturating_sub(e.dropped) as f64 / s)
        });
        *e = Sample { at: now, rx, tx, dropped, rate };
        rate
    }
}

/// Évalue les ports média maintenant (bloquant : lit sysfs).
pub fn evaluate(s: &AppState) -> Capacity {
    let cfg = HardwareConfig::load(&s.data_dir);
    let r = &cfg.network;
    let redundant = r.media_mode == MediaMode::Redundant;
    let nics = net::nics();
    let mtl = if r.media_io == MediaIo::Dpdk { s.st2110.port_stats() } else { Vec::new() };
    let mut ports = Vec::new();
    for (i, iface) in [&r.media1, &r.media2].into_iter().enumerate() {
        if iface.is_empty() {
            continue;
        }
        let nic = nics.iter().find(|n| &n.name == iface);
        let speed = {
            let mut known = s.port_meter.speeds.lock().unwrap();
            match nic.and_then(|n| n.speed) {
                Some(v) => {
                    known.insert(iface.clone(), v);
                    Some(v)
                }
                // En DPDK, la dernière vitesse vue par le noyau.
                None if r.media_io == MediaIo::Dpdk => known.get(iface).copied(),
                None => None,
            }
        };
        let counters = if r.media_io == MediaIo::Dpdk {
            mtl.get(i).copied().flatten().map(|p| (p.rx_bytes, p.tx_bytes, p.rx_dropped))
        } else {
            match (sysfs(iface, "rx_bytes"), sysfs(iface, "tx_bytes")) {
                (Some(rx), Some(tx)) => Some((rx, tx, sysfs(iface, "rx_dropped").unwrap_or(0))),
                _ => None,
            }
        };
        let rate = s.port_meter.rate(iface, counters);
        // Les flux réglés et actifs qui passent par ce port.
        let mut planned = Both::default();
        for f in cfg.flows.iter().filter(|f| f.enabled) {
            let here = if redundant { i == 0 || f.blue.is_some() } else { (f.port.clamp(1, 2) - 1) as usize == i };
            if !here {
                continue;
            }
            let m = flow_mbps(&cfg, f);
            match f.direction {
                Direction::Input => planned.rx += m,
                Direction::Output => planned.tx += m,
            }
        }
        let measured = rate.map(|r| r.0);
        let used = Both { rx: planned.rx.max(measured.map_or(0.0, |m| m.rx)), tx: planned.tx.max(measured.map_or(0.0, |m| m.tx)) };
        let free = speed.map(|v| {
            let budget = v as f64 * HEADROOM;
            Both { rx: (budget - used.rx).max(0.0), tx: (budget - used.tx).max(0.0) }
        });
        let percent = speed.filter(|v| *v > 0).map(|v| used.rx.max(used.tx) / v as f64 * 100.0);
        ports.push(PortLoad {
            n: i as u8 + 1,
            iface: iface.clone(),
            speed,
            up: nic.and_then(|n| n.up),
            measured,
            dropped: rate.map(|r| r.1),
            planned,
            used,
            free,
            percent,
        });
    }
    let formats = room(&cfg, redundant, &ports);
    let house = cfg.video_of(&Flow { format: String::new(), ..blank() }).map(crate::nmos::sdp::format_name);
    Capacity { redundant, house, headroom: HEADROOM, ports, formats }
}

/// Combien de flux de chaque format tiennent encore.
fn room(cfg: &HardwareConfig, redundant: bool, ports: &[PortLoad]) -> Vec<FormatRoom> {
    let mut list: Vec<(String, f64)> = Vec::new();
    // Le format maison d'abord (en 1080).
    let house = cfg.video_of(&Flow { format: String::new(), ..blank() });
    let names = ["1080i50", "1080i59.94", "1080p50", "1080p59.94", "720p50", "720p59.94", "2160p50", "2160p59.94"];
    if let Some((w, h, r, i)) = house {
        let name = super::super::nmos::sdp::format_name((w, h, r, i));
        if !names.contains(&name.as_str()) {
            list.push((name, video_mbps(w, h, r)));
        }
    }
    for n in names {
        if let Some((w, h, r, _)) = parse_format(n) {
            list.push((n.to_string(), video_mbps(w, h, r)));
        }
    }
    list.push(("Audio 2 ch L24".into(), audio_mbps(2, 24, 1000)));
    list.push(("Audio 8 ch L24".into(), audio_mbps(8, 24, 1000)));
    list.push(("Audio 16 ch L24".into(), audio_mbps(16, 24, 1000)));
    let fit = |pick: fn(&Both) -> f64, mbps: f64| -> Option<u32> {
        if ports.is_empty() {
            return None;
        }
        let each: Option<Vec<u32>> = ports.iter().map(|p| p.free.map(|f| (pick(&f) / mbps).floor() as u32)).collect();
        let each = each?;
        // 2022-7 : chaque flux passe par les deux ports (le plus chargé décide) ; sinon, ils
        // s'ajoutent.
        Some(if redundant { each.into_iter().min().unwrap_or(0) } else { each.into_iter().sum() })
    };
    list.into_iter().map(|(name, mbps)| FormatRoom { rx: fit(|b| b.rx, mbps), tx: fit(|b| b.tx, mbps), name, mbps: (mbps * 10.0).round() / 10.0 }).collect()
}

fn blank() -> Flow {
    serde_json::from_value(serde_json::json!({ "name": "", "red": "239.0.0.1:5004" })).expect("a flow with defaults")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_match_the_usual_figures() {
        // 1080i50 ≈ 1,1 Gb/s ; 1080p50 ≈ 2,2 ; 2160p50 ≈ 8,9 (4:2:2 10 bits, en-têtes compris).
        let r = |n: &str| {
            let (w, h, rate, _) = parse_format(n).unwrap();
            video_mbps(w, h, rate)
        };
        assert!((1100.0..1130.0).contains(&r("1080i50")), "{}", r("1080i50"));
        assert!((2200.0..2250.0).contains(&r("1080p50")), "{}", r("1080p50"));
        assert!((8850.0..8950.0).contains(&r("2160p50")), "{}", r("2160p50"));
        // Stéréo L24 en paquets de 1 ms : 2,9 Mb/s.
        assert!((2.8..3.0).contains(&audio_mbps(2, 24, 1000)));
    }

    #[test]
    fn room_in_both_modes() {
        let port = |free: f64| PortLoad { free: Some(Both { rx: free, tx: free }), ..Default::default() };
        let cfg = HardwareConfig::default();
        let one = |redundant: bool, ports: &[PortLoad]| room(&cfg, redundant, ports).into_iter().find(|f| f.name == "1080p50").unwrap().rx;
        // 25 Gb/s à 90 % : 10 flux 1080p50 par port.
        assert_eq!(one(true, &[port(22500.0), port(22500.0)]), Some(10));
        assert_eq!(one(true, &[port(22500.0), port(4500.0)]), Some(2));
        assert_eq!(one(false, &[port(22500.0), port(4500.0)]), Some(12));
        assert_eq!(one(false, &[PortLoad::default()]), None);
    }
}
