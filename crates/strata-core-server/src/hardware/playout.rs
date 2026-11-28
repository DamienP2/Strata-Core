//! Les sorties SDI, quand MXL est là : le Core envoie sur chaque connecteur réglé « Sortie » le
//! flux MXL que l'appli écrit (`sdi-out/<carte>/<n>`), en 10 bits (v210), calé sur la référence
//! de la carte (genlock) ; du noir quand aucune appli n'écrit.
//!
//! - Une sortie par fil d'exécution, tenue en accord avec les réglages (comme les entrées).
//! - Le format suit le flux : une appli qui passe de 1080i50 à 720p50 fait rouvrir la sortie.
//! - À chaque image de la carte, l'image la plus récente du flux ; s'il n'en vient plus depuis
//!   une seconde, du noir.
//! - Cartes simulées : rien n'est ouvert, l'état dit seulement si un flux arrive.
//! - Sans MXL, les sorties restent ouvertes par les applis elles-mêmes (comme avant).
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use strata_mxl::{Mxl, Video};

use super::{decklink, Function, HardwareConfig};

/// Une sortie à tenir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WantedOut {
    pub card: String,
    pub connector: u32,
    /// L'appareil Desktop Video ; None : carte simulée.
    pub device: Option<u32>,
    /// Le décalage du genlock (lignes), si la carte suit sa référence.
    pub genlock: Option<i32>,
    /// Le format du noir quand rien n'arrive : la cadence maison, en 1080.
    pub idle: Video,
}

/// Les sorties à tenir d'après les réglages et les cartes présentes.
pub fn wanted(cfg: &HardwareConfig, detected: &[decklink::DetectedCard], simulated: bool) -> Vec<WantedOut> {
    let (num, den) = cfg.clock.house.rate.ratio();
    // 25 et 29,97 : 1080i ; 24, 23,98 et 30 : 1080p.
    let interlaced = matches!((num, den), (25, 1) | (30000, 1001));
    let idle = Video { width: 1920, height: 1080, rate: (num as u32, den as u32), interlaced };
    let mut out = Vec::new();
    for c in &cfg.cards {
        let Some(d) = detected.iter().find(|d| d.id == c.id) else { continue };
        for (i, k) in c.connectors.iter().enumerate() {
            let port = d.ports.get(i).copied().unwrap_or(decklink::Port::Both);
            if k.function != Function::Output || port == decklink::Port::In {
                continue;
            }
            let Some(&device) = d.devices.get(i) else { continue };
            out.push(WantedOut {
                card: c.id.clone(),
                connector: i as u32 + 1,
                device: (!simulated).then_some(device),
                genlock: c.genlock.then_some(c.genlock_offset),
                idle,
            });
        }
    }
    out
}

#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OutState {
    #[default]
    Opening,
    /// Le flux de l'appli part sur la sortie.
    Playing,
    /// Aucune appli n'écrit : du noir.
    Black,
    Error,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct OutStatus {
    pub card: String,
    pub connector: u32,
    pub simulated: bool,
    pub state: OutState,
    /// « 1080i50 » : ce qui part sur le câble.
    pub format: Option<String>,
    pub error: Option<String>,
    /// Images envoyées depuis l'ouverture.
    pub frames: u64,
    /// Le son de l'appli arrive (sinon : du silence embarqué).
    pub audio: bool,
}

struct Running {
    wanted: WantedOut,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<OutStatus>>,
    thread: Option<JoinHandle<()>>,
}

impl Running {
    fn halt(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[derive(Default)]
pub struct Playouts {
    running: Mutex<HashMap<(String, u32), Running>>,
    mxl: Mutex<Option<Arc<Mxl>>>,
}

impl Playouts {
    pub fn set_mxl(&self, mxl: Option<Arc<Mxl>>) {
        *self.mxl.lock().unwrap() = mxl;
    }

    /// Ouvre les sorties voulues, ferme les autres. Sans MXL : aucune (les applis gardent leurs
    /// sorties).
    pub fn reconcile(&self, mut wanted: Vec<WantedOut>) {
        let mxl = self.mxl.lock().unwrap().clone();
        if mxl.is_none() {
            wanted.clear();
        }
        let mut stopped = Vec::new();
        {
            let mut run = self.running.lock().unwrap();
            let keys: Vec<_> = run.keys().cloned().collect();
            for k in keys {
                if !wanted.iter().any(|w| run[&k].wanted == *w) {
                    stopped.push(run.remove(&k).unwrap());
                }
            }
        }
        // D'abord fermées (hors du verrou) : une sortie changée (genlock, cadence) libère la carte
        // avant d'être rouverte.
        for r in stopped {
            tracing::info!("no longer playing out SDI {} of card {}", r.wanted.connector, r.wanted.card);
            r.halt();
        }
        let mut run = self.running.lock().unwrap();
        for w in wanted {
            let key = (w.card.clone(), w.connector);
            if run.contains_key(&key) {
                continue;
            }
            if let Some(m) = &mxl {
                tracing::info!("playing out SDI {} of card {}{}", w.connector, w.card, if w.device.is_none() { " (simulated)" } else { "" });
                run.insert(key, start(w, m.clone()));
            }
        }
    }

    pub fn stop_all(&self) {
        self.reconcile(Vec::new());
    }

    /// L'état d'une sortie tenue par le Core.
    pub fn status_of(&self, card: &str, connector: u32) -> Option<OutStatus> {
        self.running.lock().unwrap().get(&(card.to_string(), connector)).map(|r| r.status.lock().unwrap().clone())
    }
}

impl Drop for Playouts {
    fn drop(&mut self) {
        self.stop_all();
    }
}

pub(crate) fn format_of(v: &Video) -> String {
    let r = v.rate.0 as f64 / v.rate.1.max(1) as f64 * if v.interlaced { 2.0 } else { 1.0 };
    let r = (r * 100.0).round() / 100.0;
    format!("{}{}{}", v.height, if v.interlaced { 'i' } else { 'p' }, if r.fract() == 0.0 { format!("{r:.0}") } else { format!("{r}") })
}

/// Une image noire v210.
fn black(v: &Video) -> Vec<u8> {
    let stride = strata_mxl::v210_stride(v.width);
    let line: Vec<u8> = {
        let uyvy: Vec<u8> = (0..v.width as usize * 2).map(|i| if i % 2 == 0 { 128 } else { 16 }).collect();
        let mut l = vec![0u8; stride];
        strata_mxl::uyvy_to_v210(&uyvy, &mut l, v.width as usize);
        l
    };
    line.repeat(v.height as usize)
}

fn start(w: WantedOut, mxl: Arc<Mxl>) -> Running {
    let status = Arc::new(Mutex::new(OutStatus { card: w.card.clone(), connector: w.connector, simulated: w.device.is_none(), ..OutStatus::default() }));
    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let (w, status, stop) = (w.clone(), status.clone(), stop.clone());
        std::thread::Builder::new().name(format!("sdi-out-{}", w.connector)).spawn(move || run(&w, &mxl, &stop, &status)).ok()
    };
    Running { wanted: w, stop, status, thread }
}

fn run(w: &WantedOut, mxl: &Mxl, stop: &AtomicBool, status: &Mutex<OutStatus>) {
    let id = strata_mxl::flow_id(&format!("sdi-out/{}/{}", w.card, w.connector));
    let mut follow = strata_mxl::Follow::new(id);
    // Son son (float32 48 kHz) : embarqué dans la sortie, image par image, calé sur elle.
    let aid = strata_mxl::flow_id(&format!("sdi-out/{}/{}/audio", w.card, w.connector));
    let mut sound = Sound::default();
    let mut out: Option<(decklink_out::Out, Video)> = None;
    let mut last_new = Instant::now() - Duration::from_secs(10);
    let mut black_frame: Option<(Video, Vec<u8>)> = None;
    let mut woven: Vec<u8> = Vec::new();
    let set = |f: &dyn Fn(&mut OutStatus)| f(&mut status.lock().unwrap());
    while !stop.load(Ordering::Relaxed) {
        // Le flux de l'appli : ouvert quand il apparaît, fermé quand il disparaît, rouvert quand
        // l'appli le recrée à un autre format.
        let reader = follow.reader(mxl);
        let video = reader.map_or(w.idle, |r| r.video);
        // La sortie, au format du flux (rouverte s'il change).
        if out.as_ref().is_none_or(|(_, v)| *v != video) {
            out = None;
            match decklink_out::Out::open(w.device, &video, w.genlock) {
                Ok(o) => {
                    out = Some((o, video));
                    set(&|s| {
                        s.format = Some(format_of(&video));
                        s.error = None;
                    });
                }
                Err(e) => {
                    set(&|s| {
                        s.state = OutState::Error;
                        s.error = Some(e.clone());
                    });
                    pause(stop, Duration::from_secs(5));
                    continue;
                }
            }
        }
        let (o, v) = out.as_ref().unwrap();
        // Une image de la carte : la plus récente du flux, sinon (au-delà d'une seconde) du noir.
        o.wait(Duration::from_millis(200), v);
        let period = Duration::from_secs_f64(v.rate.1 as f64 / v.rate.0.max(1) as f64);
        let stride = strata_mxl::v210_stride(v.width);
        // Progressif : poussé à la carte droit depuis le grain MXL (sans copie) ; entrelacé : les
        // deux trames remises ensemble dans un tampon réutilisé (pas d'allocation à chaque image).
        // L'image d'abord (son instant dit où prendre le son), puis les deux ensemble à la carte.
        let channels = o.audio_channels();
        let mut heard = None;
        if let Some(r) = follow.reader(mxl) {
            if r.video.interlaced {
                if r.latest_into(period * 3, &mut woven) {
                    heard = Some(sound.frame(mxl, aid, v, r.last_frame_audio_index(), channels));
                    o.push_av(&woven, stride, &sound.pcm);
                }
            } else {
                r.with_latest(period * 3, |a, _| {
                    heard = Some(sound.frame(mxl, aid, v, r.last_frame_audio_index(), channels));
                    o.push_av(a, stride, &sound.pcm);
                });
            }
        }
        let got = heard.is_some();
        let heard = heard.unwrap_or_else(|| sound.frame(mxl, aid, v, None, channels));
        match got {
            true => {
                last_new = Instant::now();
                set(&|s| {
                    s.state = OutState::Playing;
                    s.frames += 1;
                    s.audio = heard;
                });
            }
            false if last_new.elapsed() > Duration::from_secs(1) => {
                if black_frame.as_ref().is_none_or(|(bv, _)| bv != v) {
                    black_frame = Some((*v, black(v)));
                }
                o.push_av(&black_frame.as_ref().unwrap().1, stride, &sound.pcm);
                set(&|s| {
                    s.state = OutState::Black;
                    s.audio = heard;
                });
            }
            // Un raté : la carte répète l'image d'avant.
            false => {}
        }
    }
}

/// Le son d'une sortie : continu (un nombre fixe d'échantillons par image de la carte), recalé
/// sur l'instant de l'image quand il s'en écarte de plus d'une image et demie (une image répétée
/// ne fait pas répéter le son).
#[derive(Default)]
struct Sound {
    reader: Option<strata_mxl::AudioReader>,
    checked: Option<Instant>,
    /// Le prochain échantillon à lire (48 kHz).
    cursor: Option<u64>,
    /// Le reste de la division échantillons / images (29,97 : 1601 ou 1602 par image).
    acc: u64,
    read: Vec<f32>,
    /// Ce qui part avec l'image : `channels` canaux entrelacés, 32 bits.
    pcm: Vec<i32>,
}

impl Sound {
    /// Prépare le son de la prochaine image (`at` : son instant, s'il y a une image de l'appli) ;
    /// vrai si c'est celui de l'appli (sinon du silence).
    fn frame(&mut self, mxl: &Mxl, id: strata_mxl::Uuid, v: &Video, at: Option<u64>, channels: usize) -> bool {
        // Combien d'échantillons pour cette image (48 000 × durée).
        self.acc += strata_mxl::AUDIO_RATE as u64 * v.rate.1.max(1) as u64;
        let n = (self.acc / v.rate.0.max(1) as u64) as usize;
        self.acc %= v.rate.0.max(1) as u64;
        self.pcm.clear();
        self.pcm.resize(n * channels, 0);
        if channels == 0 {
            return false;
        }
        // L'écrivain de l'appli, revu deux fois par seconde.
        if self.checked.is_none_or(|t| t.elapsed() >= Duration::from_millis(500)) {
            self.checked = Some(Instant::now());
            if mxl.is_active(id) {
                if self.reader.is_none() {
                    self.reader = mxl.audio_reader(id).ok();
                }
            } else {
                self.reader = None;
            }
        }
        let Some(r) = self.reader.as_ref() else {
            self.cursor = None;
            return false;
        };
        // Où lire : à l'instant de l'image si l'appli a écrit le son avec elle (en avance) ; sinon
        // (son écrit au fil du temps réel), ce qui vient d'être écrit, une demi-image de marge.
        // (L'appli écrit le son avec, ou juste après, l'image : quelques millisecondes de grâce.)
        let t0 = Instant::now();
        while let Some(a) = at {
            if r.written().is_some_and(|w| w + 1 >= a + n as u64) || t0.elapsed() >= Duration::from_millis(5) {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let target = at.map(|a| match r.written() {
            Some(w) if a + n as u64 > w + 1 => (w + 1).saturating_sub(n as u64 * 3 / 2),
            _ => a,
        });
        match (self.cursor, target) {
            (Some(c), Some(t)) if c.abs_diff(t) > n as u64 * 3 / 2 => self.cursor = Some(t),
            (None, Some(t)) => self.cursor = Some(t),
            _ => {}
        }
        let Some(c) = self.cursor else { return false };
        self.cursor = Some(c + n as u64);
        if !r.read_at(c, n, &mut self.read) {
            return false;
        }
        // Les canaux de l'appli rangés dans ceux de la carte (en trop : ignorés ; en moins : silence).
        let rc = r.channels.max(1);
        for k in 0..n {
            for ch in 0..channels.min(rc) {
                let x = self.read[k * rc + ch].clamp(-1.0, 1.0);
                self.pcm[k * channels + ch] = (x as f64 * i32::MAX as f64) as i32;
            }
        }
        true
    }
}

fn pause(stop: &AtomicBool, d: Duration) {
    let end = Instant::now() + d;
    while Instant::now() < end && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// La sortie DeckLink, ou son simulacre (une horloge à la cadence du flux).
mod decklink_out {
    use std::time::{Duration, Instant};
    use strata_decklink::OutputHandle;
    use strata_mxl::Video;

    pub enum Out {
        Card(OutputHandle),
        Simulated { start: Instant, n: std::cell::Cell<u64> },
    }

    impl Out {
        pub fn open(device: Option<u32>, v: &Video, genlock: Option<i32>) -> Result<Out, String> {
            match device {
                Some(d) => OutputHandle::open_v210(d, v.width, v.height, v.rate, genlock, v.interlaced).map(Out::Card),
                None => Ok(Out::Simulated { start: Instant::now(), n: std::cell::Cell::new(0) }),
            }
        }

        /// Attend la prochaine image de la carte.
        pub fn wait(&self, timeout: Duration, v: &Video) {
            match self {
                Out::Card(o) => {
                    o.wait(timeout);
                }
                Out::Simulated { start, n } => {
                    let period = Duration::from_secs_f64(v.rate.1 as f64 / v.rate.0.max(1) as f64);
                    n.set(n.get() + 1);
                    let next = *start + period * n.get() as u32;
                    let now = Instant::now();
                    if next > now {
                        std::thread::sleep((next - now).min(timeout));
                    }
                }
            }
        }

        /// Les canaux de son embarqué (simulée : 16).
        pub fn audio_channels(&self) -> usize {
            match self {
                Out::Card(o) => o.audio_channels(),
                Out::Simulated { .. } => 16,
            }
        }

        /// Une image et son son (`channels` canaux entrelacés).
        pub fn push_av(&self, frame: &[u8], stride: usize, pcm: &[i32]) {
            if let Out::Card(o) = self {
                let _ = if o.audio_channels() > 0 { o.push_av(frame, stride, pcm) } else { o.push(frame, stride) };
            }
        }
    }
}
