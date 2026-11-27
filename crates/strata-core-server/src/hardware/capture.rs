//! Les entrées SDI captées par le Core et partagées entre les applis (`strata-share`).
//!
//! Le pilote DeckLink ne laisse ouvrir une entrée qu'à un programme : le Core capte une fois
//! chaque connecteur réglé sur « Entrée » d'une carte présente, et publie ses images dans
//! `/dev/shm/strata-sdi-<carte>-<n>`. Les applis les lisent avec `strata_share::Reader`.
//!
//! - Tenu en accord avec les réglages : une entrée qui apparaît est ouverte, une entrée retirée
//!   (ou une carte enlevée) est fermée ; vérifié à chaque changement et toutes les 5 s.
//! - Une capture par fil d'exécution. Le partage existe dès le début (« pas encore de signal »),
//!   et une entrée qui ne s'ouvre pas est réessayée toutes les 5 s.
//! - Cartes simulées (`STRATA_FAKE_DECKLINK`) : une mire animée 720p25, différente par entrée.
//! - L'aperçu des réglages lit le partage comme une appli, sans s'inscrire.
use serde::Serialize;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use strata_share::{Format, FrameInfo, Reader, Timecode, Writer, UHD_UYVY};

use super::{decklink, Function, HardwareConfig};

/// Une entrée à capter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub card: String,
    /// SDI n (numéro logiciel).
    pub connector: u32,
    /// L'appareil Desktop Video ; None : simulé.
    pub device: Option<u32>,
    /// Simulée : la cadence et le timecode maison que suit la mire.
    pub pattern: Option<Pattern>,
}

/// Ce que suit la mire d'une entrée simulée : comme une caméra calée sur la maison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    pub house: strata_core::timecode::HouseClock,
    pub timezone: String,
    pub tai_offset: u32,
}

/// Les entrées à capter d'après les réglages et les cartes présentes.
pub fn wanted(cfg: &HardwareConfig, detected: &[decklink::DetectedCard], simulated: bool, timezone: &str) -> Vec<Wanted> {
    let pattern = simulated.then(|| Pattern { house: cfg.clock.house.clone(), timezone: timezone.to_string(), tai_offset: cfg.ptp.tai_offset });
    let mut out = Vec::new();
    for c in &cfg.cards {
        let Some(d) = detected.iter().find(|d| d.id == c.id) else { continue };
        for (i, k) in c.connectors.iter().enumerate() {
            let port = d.ports.get(i).copied().unwrap_or(decklink::Port::Both);
            if k.function != Function::Input || port == decklink::Port::Out {
                continue;
            }
            let Some(&device) = d.devices.get(i) else { continue };
            out.push(Wanted { card: c.id.clone(), connector: i as u32 + 1, device: (!simulated).then_some(device), pattern: pattern.clone() });
        }
    }
    out
}

#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CaptureState {
    #[default]
    Opening,
    Capturing,
    Error,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReaderApp {
    pub app: String,
    pub pid: u32,
    /// Le nom de l'appli installée de ce nom (rempli par l'API).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Où en est une entrée.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub card: String,
    pub connector: u32,
    pub simulated: bool,
    pub state: CaptureState,
    pub error: Option<String>,
    /// Un signal arrive (None : rien reçu encore).
    pub signal: Option<bool>,
    /// « 1080i50 ».
    pub format: Option<String>,
    /// Images par seconde, mesurées.
    pub fps: f64,
    pub frames: u64,
    pub timecode: Option<String>,
    /// Les applis qui la lisent.
    pub readers: Vec<ReaderApp>,
    /// Le fichier du partage.
    pub share: PathBuf,
}

struct Running {
    wanted: Wanted,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<Status>>,
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

/// Les captures en cours.
#[derive(Default)]
pub struct Captures {
    running: Mutex<HashMap<(String, u32), Running>>,
}

impl Captures {
    /// Ouvre les entrées voulues, ferme les autres (bloquant : attend la fin des fils fermés).
    pub fn reconcile(&self, wanted: Vec<Wanted>) {
        let mut stopped = Vec::new();
        {
            let mut run = self.running.lock().unwrap();
            let keys: Vec<_> = run.keys().cloned().collect();
            for k in keys {
                if !wanted.iter().any(|w| run[&k].wanted == *w) {
                    stopped.push(run.remove(&k).unwrap());
                }
            }
            for w in wanted {
                let key = (w.card.clone(), w.connector);
                if run.contains_key(&key) {
                    continue;
                }
                tracing::info!("capturing SDI {} of card {}{}", w.connector, w.card, if w.device.is_none() { " (simulated)" } else { "" });
                run.insert(key, start(w));
            }
        }
        for r in stopped {
            tracing::info!("no longer capturing SDI {} of card {}", r.wanted.connector, r.wanted.card);
            r.halt();
        }
    }

    pub fn stop_all(&self) {
        self.reconcile(Vec::new());
    }

    /// L'état de chaque entrée captée.
    pub fn list(&self) -> Vec<Status> {
        let run = self.running.lock().unwrap();
        let mut v: Vec<Status> = run.values().map(|r| r.status.lock().unwrap().clone()).collect();
        v.sort_by(|a, b| (&a.card, a.connector).cmp(&(&b.card, b.connector)));
        v
    }

    /// Le fichier du partage d'une entrée captée.
    pub fn share_of(&self, card: &str, connector: u32) -> Option<PathBuf> {
        self.running.lock().unwrap().get(&(card.to_string(), connector)).map(|r| r.status.lock().unwrap().share.clone())
    }
}

impl Drop for Captures {
    fn drop(&mut self) {
        self.stop_all();
    }
}

fn start(w: Wanted) -> Running {
    let share = strata_share::sdi_path(&w.card, w.connector);
    let status =
        Arc::new(Mutex::new(Status { card: w.card.clone(), connector: w.connector, simulated: w.device.is_none(), share: share.clone(), ..Status::default() }));
    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let (w, status, stop) = (w.clone(), status.clone(), stop.clone());
        std::thread::Builder::new()
            .name(format!("sdi-{}", w.connector))
            .spawn(move || {
                let fail = |e: String| {
                    let mut s = status.lock().unwrap();
                    s.state = CaptureState::Error;
                    s.error = Some(e);
                };
                // Le partage d'abord : les applis voient l'entrée même sans signal.
                let mut writer = loop {
                    match Writer::create(&share, 3, UHD_UYVY) {
                        Ok(wr) => break wr,
                        Err(e) => fail(format!("cannot share the input ({}): {e}", share.display())),
                    }
                    if pause(&stop, Duration::from_secs(5)) {
                        return;
                    }
                };
                match (w.device, &w.pattern) {
                    (Some(device), _) => capture(&mut writer, device, &stop, &status),
                    (None, Some(p)) => simulate(&mut writer, w.connector, p, &stop, &status),
                    (None, None) => {
                        simulate(&mut writer, w.connector, &Pattern { house: Default::default(), timezone: "UTC".into(), tai_offset: 37 }, &stop, &status)
                    }
                }
            })
            .ok()
    };
    Running { wanted: w, stop, status, thread }
}

/// Attend `d` (ou l'arrêt) : vrai si l'arrêt est demandé.
fn pause(stop: &AtomicBool, d: Duration) -> bool {
    let end = Instant::now() + d;
    while Instant::now() < end {
        if stop.load(Ordering::Relaxed) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    stop.load(Ordering::Relaxed)
}

/// Les images par seconde, et les lecteurs, mis à jour chaque seconde.
struct Meter {
    since: Instant,
    frames: u64,
}

impl Meter {
    fn new() -> Self {
        Meter { since: Instant::now(), frames: 0 }
    }

    fn tick(&mut self, got: bool, writer: &Writer, status: &Mutex<Status>, update: impl FnOnce(&mut Status)) {
        self.frames += got as u64;
        let mut s = status.lock().unwrap();
        update(&mut s);
        s.frames = writer.published();
        let dt = self.since.elapsed();
        if dt >= Duration::from_secs(1) {
            s.fps = (self.frames as f64 / dt.as_secs_f64() * 100.0).round() / 100.0;
            s.readers = writer.readers().into_iter().map(|r| ReaderApp { app: r.app, pid: r.pid, name: None }).collect();
            *self = Meter::new();
        }
    }
}

pub fn tc_text(t: &Timecode) -> String {
    format!("{:02}:{:02}:{:02}{}{:02}", t.hours, t.minutes, t.seconds, if t.drop_frame { ';' } else { ':' }, t.frames)
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// Une vraie entrée DeckLink.
fn capture(writer: &mut Writer, device: u32, stop: &AtomicBool, status: &Mutex<Status>) {
    while !stop.load(Ordering::Relaxed) {
        // 1080i50 par défaut ; une carte qui détecte le format suit le signal d'elle-même.
        let input = match strata_decklink::InputHandle::open(device, 1920, 1080, (25, 1), true) {
            Ok(h) => h,
            Err(e) => {
                let mut s = status.lock().unwrap();
                s.state = CaptureState::Error;
                s.error = Some(format!("cannot open the input: {e}"));
                drop(s);
                writer.beat();
                if pause(stop, Duration::from_secs(5)) {
                    return;
                }
                continue;
            }
        };
        {
            let mut s = status.lock().unwrap();
            s.state = CaptureState::Capturing;
            s.error = None;
        }
        let mut buf = Vec::new();
        let mut seq = 0u64;
        let mut meter = Meter::new();
        while !stop.load(Ordering::Relaxed) {
            if input.wait(seq, 200) {
                if let Some((w, h)) = input.take(&mut buf, &mut seq) {
                    let m = input.current_mode();
                    let (num, den) = (m.timescale.max(1) as u64, m.duration.max(1) as u64);
                    let g = gcd(num, den).max(1);
                    let tc = input.timecode().map(|t| Timecode {
                        hours: t.hours,
                        minutes: t.minutes,
                        seconds: t.seconds,
                        frames: t.frames,
                        drop_frame: t.drop_frame,
                        field_mark: t.field_mark,
                    });
                    let info = FrameInfo {
                        width: w,
                        height: h,
                        stride: w * 2,
                        format: Format::Uyvy,
                        rate: ((num / g) as u32, (den / g) as u32),
                        interlaced: m.interlaced(),
                        timecode: tc,
                    };
                    if writer.publish(&info, &buf).is_ok() {
                        meter.tick(true, writer, status, |s| {
                            s.format = Some(m.name.clone());
                            s.timecode = tc.as_ref().map(tc_text);
                        });
                    }
                }
            }
            let signal = input.signal();
            writer.set_signal(signal);
            meter.tick(false, writer, status, |s| {
                s.signal = signal;
                if signal == Some(false) {
                    s.fps = 0.0;
                }
            });
        }
    }
}

// ---------- La mire des cartes simulées ----------

const W: u32 = 1280;
const H: u32 = 720;

/// Les barres 75 % (BT.709, 8 bits) : Y, Cb, Cr.
const BARS: [(u8, u8, u8); 7] = [(180, 128, 128), (168, 44, 136), (145, 147, 44), (133, 63, 52), (63, 193, 204), (51, 109, 212), (28, 212, 120)];

fn put(buf: &mut [u8], x: u32, y: u32, (yy, cb, cr): (u8, u8, u8)) {
    // UYVY : Cb Y0 Cr Y1 pour deux pixels.
    let i = (y * W * 2 + (x & !1) * 2) as usize;
    buf[i] = cb;
    buf[i + 2] = cr;
    buf[i + 1 + 2 * (x & 1) as usize] = yy;
}

fn fill(buf: &mut [u8], x0: u32, y0: u32, w: u32, h: u32, c: (u8, u8, u8)) {
    for y in y0..(y0 + h).min(H) {
        for x in (x0 & !1)..(x0 + w).min(W) {
            put(buf, x, y, c);
        }
    }
}

/// Le fond : les barres, puis une bande noire ; le numéro de l'entrée en carrés blancs.
fn background(connector: u32) -> Vec<u8> {
    let mut b = vec![0u8; (W * H * 2) as usize];
    let bar = W.div_ceil(7);
    for (i, c) in BARS.iter().enumerate() {
        fill(&mut b, i as u32 * bar, 0, bar, H * 2 / 3, *c);
    }
    fill(&mut b, 0, H * 2 / 3, W, H / 3, (16, 128, 128));
    for k in 0..connector.min(16) {
        fill(&mut b, 40 + k * 44, H * 2 / 3 + 30, 30, 30, (235, 128, 128));
    }
    b
}

fn simulate(writer: &mut Writer, connector: u32, p: &Pattern, stop: &AtomicBool, status: &Mutex<Status>) {
    use strata_core::timecode::{house_timecode, HouseRate};
    let bg = background(connector);
    let mut frame = bg.clone();
    let (num, den) = p.house.rate.ratio();
    let period = Duration::from_nanos(1_000_000_000 * den / num);
    let format = format!(
        "720p{}",
        match p.house.rate {
            HouseRate::R25 => "25",
            HouseRate::R2997 => "29.97",
            HouseRate::R30 => "30",
            HouseRate::R24 => "24",
            HouseRate::R2398 => "23.98",
        }
    );
    let start = Instant::now();
    let mut meter = Meter::new();
    {
        let mut s = status.lock().unwrap();
        s.state = CaptureState::Capturing;
        s.signal = Some(true);
        s.format = Some(format);
    }
    writer.set_signal(Some(true));
    let mut n = 0u64;
    while !stop.load(Ordering::Relaxed) {
        // Une barre blanche qui traverse la bande noire, à une vitesse propre à l'entrée.
        frame.copy_from_slice(&bg);
        let speed = 6 + connector * 2;
        let x = ((n * speed as u64) % (W as u64 + 120)) as u32;
        fill(&mut frame, x.saturating_sub(120), H * 2 / 3 + 90, 120.min(x), H / 3 - 120, (235, 128, 128));
        // Le timecode maison, comme une caméra calée sur le leader.
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as i128);
        let t = house_timecode(now, &p.house, &p.timezone, p.tai_offset);
        let tc = Timecode { hours: t.hours, minutes: t.minutes, seconds: t.seconds, frames: t.frames, drop_frame: t.drop_frame, field_mark: false };
        let info =
            FrameInfo { width: W, height: H, stride: W * 2, format: Format::Uyvy, rate: (num as u32, den as u32), interlaced: false, timecode: Some(tc) };
        let ok = writer.publish(&info, &frame).is_ok();
        meter.tick(ok, writer, status, |s| s.timecode = Some(tc_text(&tc)));
        n += 1;
        let next = start + period * n as u32;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else if now - next > Duration::from_secs(1) {
            // En retard d'une seconde (machine chargée) : on repart de maintenant.
            n = ((now - start).as_nanos() / period.as_nanos()) as u64;
        }
    }
}

// ---------- L'aperçu ----------

/// L'image la plus récente d'une entrée, réduite à `width` pixels au plus, en JPEG.
pub fn preview(share: &std::path::Path, width: u32) -> Result<Vec<u8>, String> {
    let mut r = Reader::observe(share).map_err(|e| format!("cannot read the input: {e}"))?;
    let mut buf = Vec::new();
    let f = r.latest(&mut buf).ok_or("no picture yet")?;
    jpeg(&buf, f.info.width, f.info.height, f.info.stride as usize, width)
}

/// Une image UYVY réduite à `width` pixels au plus, en JPEG.
pub fn jpeg(buf: &[u8], w: u32, h: u32, stride: usize, width: u32) -> Result<Vec<u8>, String> {
    if w < 2 || h < 2 || buf.len() < stride * h as usize {
        return Err("no picture yet".into());
    }
    // Un pixel sur `step` (pair, pour garder Cb et Cr), pareil en hauteur.
    let step = w.div_ceil(width.max(16)).max(1);
    let (ow, oh) = (w / step, h / step);
    let mut rgb = Vec::with_capacity((ow * oh * 3) as usize);
    for oy in 0..oh {
        let row = &buf[(oy * step) as usize * stride..];
        for ox in 0..ow {
            let x = (ox * step) as usize;
            let p = (x & !1) * 2;
            let (cb, y, cr) = (row[p] as f32 - 128.0, row[p + 1 + 2 * (x & 1)] as f32 - 16.0, row[p + 2] as f32 - 128.0);
            // BT.709, plage limitée.
            let yy = 1.164 * y;
            let px = [yy + 1.793 * cr, yy - 0.213 * cb - 0.533 * cr, yy + 2.112 * cb];
            rgb.extend(px.iter().map(|v| v.clamp(0.0, 255.0) as u8));
        }
    }
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, 72).encode(&rgb, ow as u16, oh as u16, jpeg_encoder::ColorType::Rgb).map_err(|e| e.to_string())?;
    Ok(out)
}

/// « 1080i50 », « 720p59.94 » (en entrelacé : des trames par seconde).
pub fn format_name(info: &FrameInfo) -> String {
    let (num, den) = (info.rate.0.max(1) as f64, info.rate.1.max(1) as f64);
    let r = num / den * if info.interlaced { 2.0 } else { 1.0 };
    let r = (r * 100.0).round() / 100.0;
    let r = if r.fract() == 0.0 { format!("{r:.0}") } else { format!("{r}") };
    format!("{}{}{r}", info.height, if info.interlaced { 'i' } else { 'p' })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulated_inputs_are_shared_and_previewed() {
        let dir = std::env::temp_dir().join(format!("strata-capture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("STRATA_SHARE_DIR", &dir);
        let caps = Captures::default();
        let w = |n| Wanted { card: "abc".into(), connector: n, device: None, pattern: None };
        caps.reconcile(vec![w(1), w(2)]);
        let share = caps.share_of("abc", 2).unwrap();
        assert_eq!(share, dir.join("strata-sdi-abc-2"));
        // Le partage est créé par le fil de la capture, juste après.
        let end = Instant::now() + Duration::from_secs(3);
        while !share.exists() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut r = Reader::open(&share, "livecut").unwrap();
        let mut buf = Vec::new();
        let strata_share::Next::Frame(f) = r.next(&mut buf, Duration::from_secs(3)) else { panic!("no frame") };
        assert_eq!((f.info.width, f.info.height, f.info.rate), (1280, 720, (25, 1)));
        assert!(f.info.timecode.is_some());
        let jpeg = preview(&share, 320).unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        std::thread::sleep(Duration::from_millis(1100));
        let list = caps.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].readers, vec![ReaderApp { app: "livecut".into(), pid: std::process::id(), name: None }]);
        assert!(list[0].fps > 15.0, "{}", list[0].fps);
        // Retirée des réglages : fermée, son fichier disparaît, le lecteur le voit.
        caps.reconcile(vec![w(1)]);
        assert!(!share.exists());
        assert!(r.gone());
        caps.stop_all();
        assert!(caps.list().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
