//! Santé › Mires et analyse, le générateur : des mires (image, son, timecode) envoyées par le Core
//! vers n'importe quelle sortie — sortie SDI, flux ST 2110 envoyé, destination NDI, flux réseau —
//! ou dans un flux MXL qu'une appli lit. Chaque générateur écrit les flux MXL de sa destination,
//! que le Core envoie déjà (comme le ferait une appli) : image, son posé à l'instant de l'image
//! (calage exact), données (timecode ATC en ST 2110).
//!
//! - Réglages dans `testgen.json` ; un fil par générateur actif, tenu en accord avec eux.
//! - Une destination déjà écrite par une appli n'est pas prise : le générateur le dit.
pub mod picture;
pub mod sound;

use axum::{
    extract::{Path as UrlPath, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use strata_mxl::{Mxl, Uuid, Video};

use crate::{
    api::{bad, blocking, require, ApiResult},
    auth::CurrentUser,
    hardware::{Direction, Essence, Function, HardwareConfig},
    state::Shared,
};

/// Un générateur de mire.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Generator {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// Où l'envoyer : `sdi-out/<carte>/<n>`, `2110/<groupe>`, `ndi-out/<id>`, `stream-out/<id>` ou
    /// `mxl` (un flux MXL `testgen/<id>` pour les applis).
    pub target: String,
    /// Le format (« 1080i50 ») ; vide : la cadence maison en 1080 (un flux ST 2110 impose le sien).
    pub format: String,
    /// La mire : `bars75`, `bars100`, `smpte`, `ramp`, `checker`, `zoneplate`, `solid`, `black`,
    /// `lipsync` (éclair blanc et bip coupé, une fois par seconde), `avsync` (un curseur sur une
    /// échelle de ±500 ms, l'éclair et un bip court sur du silence : le calage de chaque canal).
    pub pattern: String,
    /// La couleur unie (`solid`), `#RRGGBB`.
    pub color: String,
    /// Le texte d'identification (vide : aucun) ; `{name}` et `{format}` y sont remplacés.
    pub ident: String,
    /// Le timecode maison incrusté.
    pub burn_tc: bool,
    /// L'heure du serveur incrustée.
    pub clock: bool,
    /// Un repère qui bouge : une image figée se voit.
    pub marker: bool,
    /// Le son : `1k`, `400`, `pink`, `silence`.
    pub tone: String,
    /// Le niveau (dBFS).
    pub level: f32,
    /// Les canaux (une destination ST 2110 impose ceux de ses flux de son).
    pub channels: u32,
    /// L'identification des canaux (le canal n coupé n fois toutes les 4 s).
    pub channel_ident: bool,
    /// Le canal n à n fois la fréquence (reconnu à l'oreille, au spectre, mesuré canal par canal).
    pub tone_per_channel: bool,
    /// Le timecode maison en ATC (flux de données d'un groupe ST 2110).
    pub atc: bool,
}

impl Default for Generator {
    fn default() -> Self {
        Generator {
            id: String::new(),
            name: String::new(),
            enabled: true,
            target: "mxl".into(),
            format: String::new(),
            pattern: "bars75".into(),
            color: "#336699".into(),
            ident: String::new(),
            burn_tc: true,
            clock: false,
            marker: true,
            tone: "1k".into(),
            level: -18.0,
            channels: 8,
            channel_ident: false,
            tone_per_channel: false,
            atc: true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct TestgenConfig {
    pub generators: Vec<Generator>,
}

fn file(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("testgen.json")
}

impl TestgenConfig {
    pub fn load(data_dir: &Path) -> TestgenConfig {
        std::fs::read_to_string(file(data_dir)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    fn save(&self, data_dir: &Path) -> std::io::Result<()> {
        let tmp = file(data_dir).with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default())?;
        std::fs::rename(tmp, file(data_dir))
    }

    /// Vérifie et nettoie les réglages.
    pub fn check(&mut self) -> Result<(), String> {
        let mut ids = std::collections::HashSet::new();
        let mut targets = std::collections::HashSet::new();
        for g in &mut self.generators {
            g.name = g.name.trim().to_string();
            g.ident = g.ident.trim().chars().take(48).collect();
            if g.id.is_empty() || g.id.len() > 64 || !g.id.chars().all(|c| c.is_ascii_alphanumeric() || "-_".contains(c)) {
                return Err(format!("{}: bad generator id", g.name));
            }
            if !ids.insert(g.id.clone()) {
                return Err(format!("two generators have the id {}", g.id));
            }
            if g.name.is_empty() {
                return Err("each generator needs a name".into());
            }
            if !picture::PATTERNS.contains(&g.pattern.as_str()) {
                return Err(format!("{}: unknown test pattern {}", g.name, g.pattern));
            }
            if !sound::TONES.contains(&g.tone.as_str()) {
                return Err(format!("{}: unknown test sound {}", g.name, g.tone));
            }
            if !g.format.is_empty() && crate::hardware::parse_format(&g.format).is_none() {
                return Err(format!("{}: unknown format {}", g.name, g.format));
            }
            if g.pattern == "solid" && picture::hex(&g.color).is_none() {
                return Err(format!("{}: the colour is #RRGGBB", g.name));
            }
            if !g.target.chars().all(|c| c.is_ascii_alphanumeric() || "-_/".contains(c)) || g.target.len() > 200 {
                return Err(format!("{}: bad destination", g.name));
            }
            if g.enabled && g.target != "mxl" && !targets.insert(g.target.clone()) {
                return Err(format!("{}: another generator already sends to this destination", g.name));
            }
            g.level = g.level.clamp(-60.0, 0.0);
            g.channels = g.channels.clamp(1, 16);
        }
        Ok(())
    }
}

/// Une destination possible : ce qu'on y écrit, et son nom.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub target: String,
    pub label: String,
    /// `sdi`, `2110`, `ndi`, `stream`, `mxl`.
    pub kind: &'static str,
    /// Le format imposé (ST 2110 : celui du flux).
    pub format: Option<String>,
    /// Les canaux imposés (ST 2110 : ceux des flux de son du groupe).
    pub channels: Option<u32>,
}

/// Le groupe d'un flux ST 2110 (lui-même s'il n'en a pas).
fn group_of(f: &crate::hardware::Flow) -> &str {
    if f.group.is_empty() {
        &f.id
    } else {
        &f.group
    }
}

/// Les destinations des générateurs d'après les réglages du serveur.
pub fn targets(hw: &HardwareConfig, streams: &crate::streams::StreamsConfig) -> Vec<Target> {
    let mut v = Vec::new();
    for c in &hw.cards {
        for (i, k) in c.connectors.iter().enumerate() {
            if k.function == Function::Output {
                let label = if k.name.is_empty() { format!("{} · SDI {}", c.name, i + 1) } else { format!("{} · SDI {} · {}", c.name, i + 1, k.name) };
                v.push(Target { target: format!("sdi-out/{}/{}", c.id, i + 1), label, kind: "sdi", format: None, channels: None });
            }
        }
    }
    for f in hw.flows.iter().filter(|f| f.direction == Direction::Output && f.essence == Essence::Video) {
        let g = group_of(f);
        let channels: u32 = hw
            .flows
            .iter()
            .filter(|a| a.direction == Direction::Output && a.essence == Essence::Audio && group_of(a) == g)
            .map(|a| a.channels as u32)
            .max()
            .unwrap_or(0);
        let format = hw.video_of(f).map(|(w, h, r, i)| crate::streams::engine::format_name(&Video { width: w, height: h, rate: r, interlaced: i }));
        v.push(Target { target: format!("2110/{g}"), label: f.name.clone(), kind: "2110", format, channels: (channels > 0).then_some(channels) });
    }
    for o in hw.ndi_outputs.iter().filter(|o| o.source.is_empty()) {
        v.push(Target { target: crate::hardware::ndi_out::out_flow(&o.id), label: o.name.clone(), kind: "ndi", format: None, channels: None });
    }
    for o in streams.outputs.iter().filter(|o| o.source.trim().is_empty()) {
        v.push(Target { target: crate::streams::output_flow(&o.id), label: o.name.clone(), kind: "stream", format: None, channels: None });
    }
    v
}

/// Les flux MXL d'une destination.
#[derive(Clone, Debug, PartialEq)]
pub struct Flows {
    pub video: (Uuid, String),
    /// Chaque flux de son et ses canaux.
    pub audio: Vec<(Uuid, String, u32)>,
    /// Le flux de données (ATC) et sa cadence.
    pub data: Option<(Uuid, String)>,
    /// Le format imposé.
    pub format: Option<Video>,
}

fn named(name: String) -> (Uuid, String) {
    (strata_mxl::flow_id(&name), name)
}

/// Les flux qu'un générateur écrit pour sa destination.
pub fn flows_of(g: &Generator, hw: &HardwareConfig) -> Result<Flows, String> {
    let plain = |base: String, channels: u32| {
        let audio = crate::streams::audio_of(&base);
        Flows { video: named(base), audio: vec![(strata_mxl::flow_id(&audio), audio, channels)], data: None, format: None }
    };
    let t = g.target.as_str();
    if t == "mxl" {
        return Ok(plain(format!("testgen/{}", g.id), g.channels));
    }
    if let Some(group) = t.strip_prefix("2110/") {
        let in_group: Vec<_> = hw.flows.iter().filter(|f| f.direction == Direction::Output && group_of(f) == group).collect();
        let video = in_group.iter().find(|f| f.essence == Essence::Video).ok_or("this ST 2110 flow is no longer sent")?;
        let format = hw.video_of(video).map(|(w, h, r, i)| Video { width: w, height: h, rate: r, interlaced: i });
        return Ok(Flows {
            video: named(format!("2110/{}", video.id)),
            audio: in_group
                .iter()
                .filter(|f| f.essence == Essence::Audio)
                .map(|f| (strata_mxl::flow_id(&format!("2110/{}", f.id)), format!("2110/{}", f.id), f.channels as u32))
                .collect(),
            data: in_group.iter().find(|f| f.essence == Essence::Data).map(|f| named(format!("2110/{}", f.id))),
            format,
        });
    }
    if t.starts_with("sdi-out/") || t.starts_with("ndi-out/") || t.starts_with("stream-out/") {
        return Ok(plain(t.to_string(), g.channels));
    }
    Err("unknown destination".into())
}

// ---------- Le moteur ----------

#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum GenState {
    #[default]
    Starting,
    Running,
    Error,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct GenStatus {
    pub state: GenState,
    pub format: Option<String>,
    pub frames: u64,
    pub error: Option<String>,
}

/// Ce que tient un générateur en marche (relancé quand ça change).
#[derive(Clone, Debug, PartialEq)]
pub struct Wanted {
    pub gen: Generator,
    pub flows: Flows,
    pub video: Video,
    pub house: strata_core::timecode::HouseClock,
    pub timezone: String,
    pub tai_offset: u32,
}

struct Running {
    wanted: Wanted,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<GenStatus>>,
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
pub struct Engine {
    running: Mutex<HashMap<String, Running>>,
    /// Les erreurs de réglage (destination disparue), par générateur.
    problems: Mutex<HashMap<String, String>>,
}

impl Engine {
    pub fn apply(&self, wanted: Vec<Wanted>, problems: HashMap<String, String>, mxl: Option<Arc<Mxl>>) {
        *self.problems.lock().unwrap() = problems;
        let wanted = if mxl.is_some() { wanted } else { Vec::new() };
        let stopped: Vec<Running> = {
            let mut run = self.running.lock().unwrap();
            let gone: Vec<String> = run.iter().filter(|(_, r)| !wanted.contains(&r.wanted)).map(|(k, _)| k.clone()).collect();
            gone.into_iter().filter_map(|k| run.remove(&k)).collect()
        };
        // Arrêtés d'abord : leurs flux sont libérés avant d'être repris.
        for r in stopped {
            r.halt();
        }
        let Some(mxl) = mxl else { return };
        let mut run = self.running.lock().unwrap();
        for w in wanted {
            if run.contains_key(&w.gen.id) {
                continue;
            }
            tracing::info!("test pattern {} started ({})", w.gen.name, w.gen.target);
            let status = Arc::new(Mutex::new(GenStatus::default()));
            let stop = Arc::new(AtomicBool::new(false));
            let thread = {
                let (w, status, stop, mxl) = (w.clone(), status.clone(), stop.clone(), mxl.clone());
                std::thread::Builder::new().name(format!("testgen-{}", w.gen.id)).spawn(move || run_gen(&w, &mxl, &stop, &status)).ok()
            };
            run.insert(w.gen.id.clone(), Running { wanted: w, stop, status, thread });
        }
    }

    pub fn stop_all(&self) {
        self.apply(Vec::new(), HashMap::new(), None);
    }

    pub fn status_of(&self, id: &str) -> Option<GenStatus> {
        if let Some(p) = self.problems.lock().unwrap().get(id) {
            return Some(GenStatus { state: GenState::Error, error: Some(p.clone()), ..GenStatus::default() });
        }
        self.running.lock().unwrap().get(id).map(|r| r.status.lock().unwrap().clone())
    }

    /// Les flux MXL écrits par les générateurs en marche (l'analyseur et l'aperçu les lisent).
    pub fn video_flow(&self, id: &str) -> Option<String> {
        self.running.lock().unwrap().get(id).map(|r| r.wanted.flows.video.1.clone())
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop_all();
    }
}

/// Tient les générateurs en accord avec leurs réglages.
pub fn reconcile(s: &crate::state::AppState) {
    let cfg = TestgenConfig::load(&s.data_dir);
    let hw = HardwareConfig::load(&s.data_dir);
    let timezone = s.config.load().timezone.clone();
    let mut wanted = Vec::new();
    let mut problems = HashMap::new();
    for g in cfg.generators.iter().filter(|g| g.enabled) {
        let flows = match flows_of(g, &hw) {
            Ok(f) => f,
            Err(e) => {
                problems.insert(g.id.clone(), e);
                continue;
            }
        };
        let video = flows
            .format
            .or_else(|| crate::hardware::parse_format(&g.format).map(|(w, h, r, i)| Video { width: w, height: h, rate: r, interlaced: i }))
            .unwrap_or_else(|| crate::streams::house_video(&hw));
        wanted.push(Wanted { gen: g.clone(), flows, video, house: hw.clock.house.clone(), timezone: timezone.clone(), tai_offset: hw.ptp.tai_offset });
    }
    s.testgen.apply(wanted, problems, crate::hardware::mxl_of(s));
}

fn now_unix_ns() -> i128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as i128)
}

fn run_gen(w: &Wanted, mxl: &Mxl, stop: &AtomicBool, status: &Mutex<GenStatus>) {
    let set = |f: &dyn Fn(&mut GenStatus)| f(&mut status.lock().unwrap());
    let format = crate::streams::engine::format_name(&w.video);
    set(&|s| s.format = Some(format.clone()));
    while !stop.load(Ordering::Relaxed) {
        let r = generate(w, mxl, stop, &set);
        if stop.load(Ordering::Relaxed) {
            break;
        }
        if let Err(e) = r {
            set(&|s| {
                s.state = GenState::Error;
                s.error = Some(e.clone());
            });
        }
        // Réessaie (une appli qui écrivait la destination a pu s'arrêter).
        let until = Instant::now() + Duration::from_secs(3);
        while !stop.load(Ordering::Relaxed) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

type Setter<'a> = &'a dyn Fn(&dyn Fn(&mut GenStatus));

fn generate(w: &Wanted, mxl: &Mxl, stop: &AtomicBool, set: Setter) -> Result<(), String> {
    let g = &w.gen;
    let v = w.video;
    // Une destination déjà écrite (une appli) n'est pas prise.
    if mxl.is_active(w.flows.video.0) {
        return Err("an app already writes this destination".into());
    }
    let label = format!("Mire {} (Strata Core)", g.name);
    let mut vw = mxl.video_writer(w.flows.video.0, &label, &v)?;
    let mut audio: Vec<(strata_mxl::AudioWriter, sound::Sound)> = Vec::new();
    for (id, _, ch) in &w.flows.audio {
        if mxl.is_active(*id) {
            continue;
        }
        let aw = mxl.audio_writer(*id, &label, *ch)?;
        audio.push((aw, sound::Sound::new(&g.tone, g.level, *ch as usize, g.channel_ident).beeping(g.pattern == "avsync").per_channel(g.tone_per_channel)));
    }
    let grains = if v.interlaced { (v.rate.0 * 2, v.rate.1) } else { v.rate };
    let mut data = match (&w.flows.data, g.atc) {
        (Some((id, _)), true) if !mxl.is_active(*id) => Some(mxl.data_writer(*id, &label, grains)?),
        _ => None,
    };
    let (width, height) = (v.width as usize, v.height as usize);
    let base = picture::pattern(&g.pattern, width, height, &g.color);
    let mut frame = picture::Frame::new(width, height, picture::grey(0.0));
    let s = picture::scale(height);
    let format_name = crate::streams::engine::format_name(&v);
    let ident = g.ident.replace("{name}", &g.name).replace("{format}", &format_name);
    let av = picture::av_scale(width, height);
    let avsync = g.pattern == "avsync";
    let (num, den) = (v.rate.0.max(1) as u64, v.rate.1.max(1) as u64);
    let per_second = num.div_ceil(den);
    let start = Instant::now();
    let mut acc = 0u64;
    let mut tick = Instant::now();
    for n in 0u64.. {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let tc = strata_core::timecode::house_timecode(now_unix_ns(), &w.house, &w.timezone, w.tai_offset);
        // La mire de calage : un éclair blanc la première image de chaque seconde.
        let flash = (g.pattern == "lipsync" || avsync) && tc.frames == 0;
        if flash && !avsync {
            frame.fill(0, 0, width, height, picture::grey(1.0));
        } else {
            frame.data.copy_from_slice(&base.data);
        }
        if avsync {
            // Le curseur : au 0 à l'éclair, à droite après (le son qui arrive en retard), à gauche
            // avant.
            let t = tc.frames as i64 * 1000 / per_second.max(1) as i64;
            let rel = if t < 500 { t } else { t - 1000 };
            let x = av.x0 as i64 + (av.x1 - av.x0) as i64 * (rel + 500) / 1000;
            let x = x.max(0) as usize;
            frame.fill(x.saturating_sub(s), av.y.saturating_sub(av.tick * 4), x + s, av.y + av.tick * 2, picture::ycc(1.0, 0.8, 0.0));
            if flash {
                // L'éclair : un carré blanc au centre (là où l'analyseur regarde).
                let half = height / 8;
                frame.fill(width / 2 - half, height / 2 - half, width / 2 + half, height / 2 + half, picture::grey(1.0));
            }
        }
        if g.pattern == "lipsync" && !flash {
            // Un trait qui avance d'un bord à l'autre en une seconde : on voit où en est l'éclair.
            let x = width * tc.frames as usize / per_second.max(1) as usize;
            frame.fill(x, height / 3, x + 2 * s, height * 2 / 3, picture::grey(0.9));
        }
        if g.marker && !flash {
            // Un carré qui fait l'aller en deux secondes, en bas : une image figée se voit.
            let period = 2 * per_second;
            let pos = (n % period) as usize;
            let x = (width - 6 * s) * pos / period as usize;
            frame.fill(x, height - 14 * s, x + 6 * s, height - 8 * s, picture::grey(1.0));
        }
        // L'éclair reste blanc partout : c'est lui que l'analyseur cherche.
        if !ident.is_empty() && (!flash || avsync) {
            let y = if avsync { height / 6 } else { height / 2 - 4 * s };
            picture::label_centred(&mut frame, y, &ident, s);
        }
        if g.burn_tc && (!flash || avsync) {
            picture::label_centred(&mut frame, height - 28 * s, &format!("TC {tc}"), s);
        }
        if g.clock && (!flash || avsync) {
            let t = jiff::Timestamp::now().to_zoned_datetime(jiff::tz::TimeZone::get(&w.timezone).unwrap_or(jiff::tz::TimeZone::UTC));
            let text = format!("{:02}:{:02}:{:02}", t.hour(), t.minute(), t.second());
            let x = width.saturating_sub(picture::text_width(&text, s) + 8 * s);
            picture::label(&mut frame, x, 8 * s, &text, s);
        }
        vw.write_uyvy(&frame.data, frame.stride())?;
        // Le son de cette image, posé à son instant (calage exact) ; silence pendant l'éclair.
        acc += sound::RATE * den;
        let k = (acc / num) as usize;
        acc %= num;
        if let Some(at) = vw.last_frame_audio_index() {
            for (aw, snd) in audio.iter_mut() {
                let pcm = snd.next(k, flash);
                aw.write_at(at, &pcm)?;
            }
        }
        if let Some(d) = data.as_mut() {
            let p = strata_mxl::anc::atc_packet(tc.hours, tc.minutes, tc.seconds, tc.frames, tc.drop_frame);
            if v.interlaced {
                d.write(std::slice::from_ref(&p), 2)?;
                d.write(&[], 3)?;
            } else {
                d.write(std::slice::from_ref(&p), 0)?;
            }
        }
        if tick.elapsed() >= Duration::from_secs(1) {
            tick = Instant::now();
            set(&|st| {
                st.state = GenState::Running;
                st.frames = n + 1;
                st.error = None;
            });
        }
        let due = start + Duration::from_nanos((n + 1) * 1_000_000_000 * den / num);
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
    }
    Ok(())
}

// ---------- L'API ----------

pub fn routes() -> Router<Shared> {
    Router::new().route("/api/testgen", get(read).put(write)).route("/api/testgen/{id}/preview.jpg", get(preview))
}

fn view(s: &crate::state::AppState) -> Value {
    let cfg = TestgenConfig::load(&s.data_dir);
    let hw = HardwareConfig::load(&s.data_dir);
    let streams = crate::streams::StreamsConfig::load(&s.data_dir);
    let gens: Vec<Value> = cfg
        .generators
        .iter()
        .map(|g| {
            let mut v = json!(g);
            v["status"] = json!(if g.enabled { s.testgen.status_of(&g.id) } else { None });
            v["flow"] = json!(flows_of(g, &hw).ok().map(|f| f.video.0.to_string()));
            v
        })
        .collect();
    json!({
        "generators": gens,
        "targets": targets(&hw, &streams),
        "house": crate::streams::engine::format_name(&crate::streams::house_video(&hw)),
        "mxl": crate::hardware::mxl_of(s).is_some(),
    })
}

async fn read(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let s2 = s.clone();
    Ok(Json(blocking(move || view(&s2)).await?))
}

async fn write(State(s): State<Shared>, CurrentUser(me): CurrentUser, Json(mut cfg): Json<TestgenConfig>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    cfg.check().map_err(bad)?;
    let s2 = s.clone();
    let v = blocking(move || {
        cfg.save(&s2.data_dir).map_err(|e| e.to_string())?;
        reconcile(&s2);
        Ok::<_, String>(view(&s2))
    })
    .await?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    tracing::info!("test patterns changed by {}", me.username);
    Ok(Json(v))
}

async fn preview(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath(id): UrlPath<String>) -> ApiResult<impl IntoResponse> {
    require(me.admin)?;
    let s2 = s.clone();
    let jpeg = blocking(move || {
        let cfg = TestgenConfig::load(&s2.data_dir);
        let hw = HardwareConfig::load(&s2.data_dir);
        let g = cfg.generators.iter().find(|g| g.id == id).ok_or("no such generator")?;
        let flows = flows_of(g, &hw)?;
        let mxl = crate::hardware::mxl_of(&s2).ok_or("no MXL")?;
        s2.previews.jpeg(&format!("testgen/{id}"), || crate::hardware::preview::Feed::Mxl { mxl, id: flows.video.0, reader: None })
    })
    .await?
    .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    Ok(([(axum::http::header::CONTENT_TYPE, "image/jpeg"), (axum::http::header::CACHE_CONTROL, "no-store")], (*jpeg).clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gen(id: &str, target: &str) -> Generator {
        Generator { id: id.into(), name: id.to_uppercase(), target: target.into(), ..Generator::default() }
    }

    #[test]
    fn checks() {
        let mut c = TestgenConfig { generators: vec![gen("a", "mxl"), gen("b", "mxl")] };
        c.check().unwrap();
        let mut c = TestgenConfig { generators: vec![gen("a", "sdi-out/x/1"), gen("b", "sdi-out/x/1")] };
        assert!(c.check().is_err(), "same destination");
        let mut c = TestgenConfig { generators: vec![Generator { pattern: "rainbow".into(), ..gen("a", "mxl") }] };
        assert!(c.check().is_err());
        let mut c = TestgenConfig { generators: vec![Generator { level: 6.0, channels: 99, ..gen("a", "mxl") }] };
        c.check().unwrap();
        assert_eq!((c.generators[0].level, c.generators[0].channels), (0.0, 16));
    }

    #[test]
    fn flows_of_destinations() {
        let hw = HardwareConfig::default();
        let f = flows_of(&gen("pgm", "mxl"), &hw).unwrap();
        assert_eq!(f.video.1, "testgen/pgm");
        assert_eq!(f.audio[0].1, "testgen/pgm/audio");
        let f = flows_of(&gen("x", "sdi-out/card/3"), &hw).unwrap();
        assert_eq!(f.video.0, strata_mxl::flow_id("sdi-out/card/3"));
        assert!(flows_of(&gen("x", "2110/nope"), &hw).is_err());
    }
}
