//! Santé › Mires et analyse, l'analyseur : n'importe quelle source du serveur (entrée SDI, flux
//! ST 2110 reçu avec ou sans image, flux réseau reçu, et ce qui part : sorties, destinations,
//! mires) analysée par le Core pendant qu'on la regarde — scopes, son, sonie R128, calage
//! image/son, données — et écoutée dans le navigateur (tous les canaux, ou ceux en solo).
//!
//! - Une session par source, lancée à la première demande, arrêtée 15 s après la dernière (et
//!   sans personne qui écoute).
//! - L'écoute : du PCM 16 bits stéréo 48 kHz en flux HTTP continu (le navigateur le joue).
//! - Surveillée en continu (`watch`) : la session ne s'arrête plus, son historique et son journal
//!   continuent sans personne devant (jusqu'au redémarrage du Core ou à l'arrêt).
pub mod audio;
pub mod history;
pub mod video;

use history::{History, Level};

use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use strata_mxl::Mxl;

use crate::{
    api::{blocking, require, ApiResult},
    auth::CurrentUser,
    hardware::{Direction, Essence, Function, HardwareConfig},
    state::Shared,
};

/// Une source analysable.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub key: String,
    pub label: String,
    /// `sdi`, `2110`, `audio` (son seul), `stream`, `ndi`, `mxl`.
    pub kind: &'static str,
    /// Reçue (`in`) ou envoyée (`out`).
    pub way: &'static str,
    pub video: Option<String>,
    pub audio: Vec<String>,
    pub data: Option<String>,
}

fn group_of(f: &crate::hardware::Flow) -> &str {
    if f.group.is_empty() {
        &f.id
    } else {
        &f.group
    }
}

/// Les sources du serveur.
pub fn sources(s: &crate::state::AppState) -> Vec<Source> {
    let hw = HardwareConfig::load(&s.data_dir);
    let streams = crate::streams::StreamsConfig::load(&s.data_dir);
    let testgen = crate::testgen::TestgenConfig::load(&s.data_dir);
    let mut v = Vec::new();
    let plain = |key: String, label: String, kind: &'static str, way: &'static str, base: String| Source {
        key,
        label,
        kind,
        way,
        audio: vec![crate::streams::audio_of(&base)],
        video: Some(base),
        data: None,
    };
    for c in &hw.cards {
        for (i, k) in c.connectors.iter().enumerate() {
            let name = if k.name.is_empty() { format!("{} · SDI {}", c.name, i + 1) } else { format!("{} · SDI {} · {}", c.name, i + 1, k.name) };
            match k.function {
                Function::Input => v.push(plain(format!("sdi/{}/{}", c.id, i + 1), name, "sdi", "in", format!("sdi/{}/{}", c.id, i + 1))),
                Function::Output => v.push(plain(format!("sdi-out/{}/{}", c.id, i + 1), name, "sdi", "out", format!("sdi-out/{}/{}", c.id, i + 1))),
                Function::Unused => {}
            }
        }
    }
    // Les flux ST 2110, groupe par groupe : avec une vidéo, ou du son seul.
    let mut seen = std::collections::HashSet::new();
    for f in hw.flows.iter().filter(|f| f.enabled) {
        let (g, dir) = (group_of(f).to_string(), f.direction);
        if !seen.insert((g.clone(), dir == Direction::Input)) {
            continue;
        }
        let members: Vec<_> = hw.flows.iter().filter(|x| x.enabled && x.direction == dir && group_of(x) == g).collect();
        let video = members.iter().find(|x| x.essence == Essence::Video);
        let label = video.map(|x| x.name.clone()).unwrap_or_else(|| members.first().map(|x| x.name.clone()).unwrap_or_default());
        v.push(Source {
            key: format!("2110{}/{g}", if dir == Direction::Input { "in" } else { "out" }),
            label,
            kind: if video.is_some() { "2110" } else { "audio" },
            way: if dir == Direction::Input { "in" } else { "out" },
            video: video.map(|x| format!("2110/{}", x.id)),
            audio: members.iter().filter(|x| x.essence == Essence::Audio).map(|x| format!("2110/{}", x.id)).collect(),
            data: members.iter().find(|x| x.essence == Essence::Data).map(|x| format!("2110/{}", x.id)),
        });
    }
    for i in &streams.inputs {
        v.push(plain(format!("stream-in/{}", i.id), i.name.clone(), "stream", "in", crate::streams::input_flow(&i.id)));
    }
    for o in streams.outputs.iter().filter(|o| o.source.trim().is_empty()) {
        v.push(plain(format!("stream-out/{}", o.id), o.name.clone(), "stream", "out", crate::streams::output_flow(&o.id)));
    }
    for o in hw.ndi_outputs.iter().filter(|o| o.source.is_empty()) {
        v.push(plain(format!("ndi-out/{}", o.id), o.name.clone(), "ndi", "out", o.flow()));
    }
    for g in testgen.generators.iter().filter(|g| g.target == "mxl") {
        v.push(plain(format!("testgen/{}", g.id), g.name.clone(), "mxl", "out", format!("testgen/{}", g.id)));
    }
    // Ce qui est reçu d'abord (ce qu'on vérifie le plus souvent).
    v.sort_by_key(|s| s.way != "in");
    v
}

// ---------- Les sessions ----------

struct Listener {
    solo: Vec<usize>,
    tx: tokio::sync::mpsc::Sender<axum::body::Bytes>,
}

struct Session {
    source: Source,
    stop: AtomicBool,
    seen: Mutex<Instant>,
    snap: Mutex<Value>,
    reset: AtomicBool,
    /// Les canaux de la sonie demandés (0 = le premier) ; None : 1-2 (ou 1).
    group: Mutex<Option<Vec<usize>>>,
    listeners: Mutex<Vec<Listener>>,
    done: AtomicBool,
    history: Mutex<History>,
    /// Surveillée en continu : pas d'arrêt faute de regard.
    pinned: AtomicBool,
}

#[derive(Default)]
pub struct Analyzer {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

impl Analyzer {
    fn session(&self, src: Source, mxl: Arc<Mxl>, app: Shared) -> Arc<Session> {
        let mut map = self.sessions.lock().unwrap();
        map.retain(|_, s| !s.done.load(Ordering::Relaxed));
        if let Some(s) = map.get(&src.key).filter(|s| s.source == src) {
            *s.seen.lock().unwrap() = Instant::now();
            return s.clone();
        }
        if let Some(old) = map.remove(&src.key) {
            old.stop.store(true, Ordering::Relaxed);
        }
        let s = Arc::new(Session {
            source: src.clone(),
            stop: AtomicBool::new(false),
            seen: Mutex::new(Instant::now()),
            snap: Mutex::new(json!({ "starting": true })),
            reset: AtomicBool::new(false),
            group: Mutex::new(None),
            listeners: Mutex::new(Vec::new()),
            done: AtomicBool::new(false),
            history: Mutex::new(History::default()),
            pinned: AtomicBool::new(false),
        });
        let s2 = s.clone();
        let _ = std::thread::Builder::new().name(format!("analyze-{}", src.key)).spawn(move || {
            run(&s2, &mxl, &app);
            s2.done.store(true, Ordering::Relaxed);
        });
        map.insert(src.key, s.clone());
        s
    }

    /// Les sources surveillées en continu.
    pub fn watched(&self) -> Vec<String> {
        let map = self.sessions.lock().unwrap();
        let mut v: Vec<String> =
            map.iter().filter(|(_, s)| s.pinned.load(Ordering::Relaxed) && !s.done.load(Ordering::Relaxed)).map(|(k, _)| k.clone()).collect();
        v.sort();
        v
    }

    pub fn stop_all(&self) {
        for s in self.sessions.lock().unwrap().drain().map(|(_, s)| s) {
            s.stop.store(true, Ordering::Relaxed);
        }
    }
}

/// Une mesure du calage : l'éclair (l'index de son de son image) et le repère du son le plus
/// proche, jusqu'à ±400 ms.
const LIP_SPAN: u64 = 19_200;

/// Au-delà, le calage est signalé dans le journal (ms).
const LIP_TOLERANCE_MS: f64 = 5.0;

/// Les repères du son d'un canal pour le calage : où le son se coupe (mire Calage : un son
/// continu coupé à l'éclair) et où il reprend après un silence (mire Synchro AV : un bip à
/// l'éclair). Le canal surtout sonore se mesure sur ses coupures, le canal surtout muet sur ses
/// bips.
#[derive(Default)]
struct Edges {
    zero_run: u64,
    zero_start: u64,
    sounding: bool,
    cuts: VecDeque<u64>,
    beeps: VecDeque<u64>,
    /// La part du temps où le canal sonne (lissée).
    tonal: f32,
}

impl Edges {
    fn push(&mut self, at: u64, x: f32) {
        if x.abs() < 1e-5 {
            if self.zero_run == 0 {
                self.zero_start = at;
            }
            self.zero_run += 1;
            if self.zero_run == 24 && self.sounding {
                self.cuts.push_back(self.zero_start);
                self.sounding = false;
            }
            self.tonal *= 0.999_98;
        } else {
            if self.zero_run >= 240 {
                self.beeps.push_back(at);
            }
            self.zero_run = 0;
            self.sounding = true;
            self.tonal = self.tonal * 0.999_98 + 0.000_02;
        }
        while self.cuts.len() > 8 {
            self.cuts.pop_front();
        }
        while self.beeps.len() > 8 {
            self.beeps.pop_front();
        }
    }

    /// Le décalage (ms) du repère le plus proche de l'éclair `at`.
    fn offset(&self, at: u64) -> Option<f64> {
        let list = if self.tonal > 0.5 { &self.cuts } else { &self.beeps };
        list.iter().copied().filter(|o| o.abs_diff(at) <= LIP_SPAN).min_by_key(|o| o.abs_diff(at)).map(|o| (o as f64 - at as f64) / 48.0)
    }
}

/// Le stéréo de l'écoute : tous les canaux (impairs à gauche, pairs à droite), ou ceux en solo
/// (un seul : aux deux oreilles).
pub fn listen_mix(frame: &[f32], solo: &[usize]) -> (f32, f32) {
    let pick: Vec<usize> = if solo.is_empty() { (0..frame.len()).collect() } else { solo.iter().copied().filter(|c| *c < frame.len()).collect() };
    match pick.len() {
        0 => (0.0, 0.0),
        1 => (frame[pick[0]], frame[pick[0]]),
        n => {
            let (mut l, mut r) = (0.0, 0.0);
            for (i, c) in pick.iter().enumerate() {
                if i % 2 == 0 {
                    l += frame[*c];
                } else {
                    r += frame[*c];
                }
            }
            let g = 1.0 / ((n as f32 / 2.0).max(1.0)).sqrt();
            ((l * g).clamp(-1.0, 1.0), (r * g).clamp(-1.0, 1.0))
        }
    }
}

/// Les mesures d'une seconde, accumulées entre deux échantillons de l'historique.
#[derive(Default)]
struct Second {
    peak: Option<f32>,
    clip: bool,
    silent: Vec<bool>,
}

/// Le suivi IP d'un flux ST 2110 (les compteurs de la seconde d'avant).
struct IpTrack {
    id: String,
    name: String,
    lost: Option<u64>,
    unrecovered: Option<u64>,
}

fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

fn run(s: &Session, mxl: &Mxl, app: &Shared) {
    let src = &s.source;
    let mut sec = Second::default();
    let mut sampled = Instant::now();
    let mut last_format: Option<String> = None;
    let started = Instant::now();
    let hw = HardwareConfig::load(&app.data_dir);
    let mut ips: Vec<IpTrack> = src
        .video
        .iter()
        .chain(src.audio.iter())
        .chain(src.data.iter())
        .filter_map(|n| n.strip_prefix("2110/"))
        .filter_map(|id| hw.flows.iter().find(|f| f.id == id))
        .map(|f| IpTrack { id: f.id.clone(), name: f.name.clone(), lost: None, unrecovered: None })
        .collect();
    let video_flow = src.video.as_deref().and_then(|n| n.strip_prefix("2110/")).map(str::to_string);
    drop(hw);
    s.history.lock().unwrap().event(now_ms(), Level::Info, "started", json!({ "source": src.label }));
    let vid = src.video.as_deref().map(strata_mxl::flow_id);
    let aids: Vec<strata_mxl::Uuid> = src.audio.iter().map(|a| strata_mxl::flow_id(a)).collect();
    let did = src.data.as_deref().map(strata_mxl::flow_id);
    let mut vr: Option<strata_mxl::VideoReader> = None;
    let mut ars: Vec<Option<strata_mxl::AudioReader>> = aids.iter().map(|_| None).collect();
    let mut dr: Option<strata_mxl::DataReader> = None;
    let mut meter: Option<audio::Meter> = None;
    let mut pos: Option<u64> = None;
    let mut checked = Instant::now() - Duration::from_secs(5);
    let mut snapped = Instant::now();
    let mut scoped = Instant::now() - Duration::from_secs(1);
    let mut levels: Vec<audio::Level> = Vec::new();
    let mut scopes: Option<video::Scopes> = None;
    let mut frame = Vec::new();
    let (mut last_sig, mut still_since): (Option<video::Signature>, Option<Instant>) = (None, None);
    let (mut last_head, mut was_flash) = (None, false);
    let mut flashes: VecDeque<u64> = VecDeque::new();
    let mut edges: Vec<Edges> = Vec::new();
    let mut per_channel: Vec<Option<f64>> = Vec::new();
    let mut lip: VecDeque<f64> = VecDeque::new();
    let mut lip_seen: Option<Instant> = None;
    let (mut tc, mut captions, mut afd, mut scte): (Option<String>, Option<Instant>, Option<u8>, Option<String>) = (None, None, None, None);
    let mut buf = Vec::new();
    while !s.stop.load(Ordering::Relaxed) {
        let idle = s.seen.lock().unwrap().elapsed() > Duration::from_secs(15);
        let listening = {
            let mut l = s.listeners.lock().unwrap();
            l.retain(|x| !x.tx.is_closed());
            !l.is_empty()
        };
        if idle && !listening && !s.pinned.load(Ordering::Relaxed) {
            break;
        }
        // Les flux, revus chaque seconde.
        if checked.elapsed() >= Duration::from_secs(1) {
            checked = Instant::now();
            if let Some(id) = vid {
                if !mxl.is_active(id) {
                    vr = None;
                } else if vr.as_ref().is_none_or(|r| mxl.stale(id, r.stamp)) {
                    drop(vr.take());
                    vr = mxl.video_reader(id).ok();
                }
            }
            for (i, id) in aids.iter().enumerate() {
                if !mxl.is_active(*id) {
                    ars[i] = None;
                } else if ars[i].as_ref().is_none_or(|r| mxl.stale(*id, r.stamp)) {
                    drop(ars[i].take());
                    ars[i] = mxl.audio_reader(*id).ok();
                }
            }
            if let Some(id) = did {
                if !mxl.is_active(id) {
                    dr = None;
                } else if dr.as_ref().is_none_or(|r| mxl.stale(id, r.stamp)) {
                    drop(dr.take());
                    dr = mxl.data_reader(id).ok();
                }
            }
            let total: usize = ars.iter().flatten().map(|r| r.channels).sum();
            if total == 0 {
                meter = None;
                pos = None;
            } else if meter.as_ref().is_none_or(|m| m.channels != total) {
                meter = Some(audio::Meter::new(total, if total >= 2 { vec![0, 1] } else { vec![0] }));
            }
        }
        if let (Some(m), Some(g)) = (meter.as_mut(), s.group.lock().unwrap().clone()) {
            let g: Vec<usize> = g.into_iter().filter(|c| *c < m.channels).collect();
            if !g.is_empty() && g != m.group() {
                m.set_group(g);
            }
        }
        if s.reset.swap(false, Ordering::Relaxed) {
            if let Some(m) = meter.as_mut() {
                m.reset();
            }
            lip.clear();
            per_channel.iter_mut().for_each(|o| *o = None);
        }
        // L'image : l'éclair de la mire de calage à chaque image, les scopes 5 fois par seconde.
        if let Some(r) = vr.as_ref() {
            let v = r.video;
            let period = Duration::from_secs_f64(v.rate.1.max(1) as f64 / v.rate.0.max(1) as f64);
            let head = r.head();
            if head != last_head {
                last_head = head;
                let stride = strata_mxl::v210_stride(v.width);
                let row = v.grain_lines() as usize / 2;
                if let Some(l) = r.with_latest(period * 3, |a, _| video::luma_at(&a[row * stride..(row + 1) * stride], v.width as usize / 2)) {
                    let flash = l > 800;
                    if flash && !was_flash {
                        if let Some(at) = r.last_frame_audio_index() {
                            flashes.push_back(at);
                        }
                    }
                    was_flash = flash;
                }
            }
            if scoped.elapsed() >= Duration::from_millis(200) {
                scoped = Instant::now();
                if r.latest_into(period * 3, &mut frame) {
                    let (sc, sig) = video::scopes(&frame, strata_mxl::v210_stride(v.width), v.width as usize, v.height as usize);
                    let same = last_sig.as_ref().is_some_and(|p| video::difference(p, &sig) < 0.5);
                    if same {
                        still_since.get_or_insert(Instant::now());
                    } else {
                        still_since = None;
                    }
                    last_sig = Some(sig);
                    scopes = Some(sc);
                } else {
                    scopes = None;
                }
            }
        } else {
            scopes = None;
        }
        // Le son : tous les flux lus ensemble, du même instant (calés par leurs horodatages).
        if let Some(m) = meter.as_mut() {
            let readers: Vec<&strata_mxl::AudioReader> = ars.iter().flatten().collect();
            let written = readers.iter().filter_map(|r| r.written()).min();
            if let Some(w) = written {
                let p = *pos.get_or_insert(w.saturating_sub(4_800));
                let p = if w.saturating_sub(p) > 9_600 { w.saturating_sub(4_800) } else { p };
                let n = (w + 1).saturating_sub(p).min(2_400) as usize;
                if n >= 240 {
                    let total = m.channels;
                    let mut chunk = vec![0f32; n * total];
                    let mut ok = true;
                    let mut base = 0;
                    for r in &readers {
                        if !r.read_at(p, n, &mut buf) {
                            ok = false;
                            break;
                        }
                        for k in 0..n {
                            for c in 0..r.channels {
                                chunk[k * total + base + c] = buf[k * r.channels + c];
                            }
                        }
                        base += r.channels;
                    }
                    if ok {
                        m.push(&chunk);
                        // Les repères de chaque canal (calage canal par canal).
                        if edges.len() != total {
                            edges = (0..total).map(|_| Edges { tonal: 1.0, ..Edges::default() }).collect();
                            per_channel = vec![None; total];
                        }
                        for k in 0..n {
                            for (c, e) in edges.iter_mut().enumerate() {
                                e.push(p + k as u64, chunk[k * total + c]);
                            }
                        }
                        // L'écoute.
                        let ls = s.listeners.lock().unwrap();
                        for l in ls.iter() {
                            let mut out = Vec::with_capacity(n * 4);
                            for f in chunk.chunks_exact(total) {
                                let (a, b) = listen_mix(f, &l.solo);
                                out.extend_from_slice(&((a * 32767.0) as i16).to_le_bytes());
                                out.extend_from_slice(&((b * 32767.0) as i16).to_le_bytes());
                            }
                            let _ = l.tx.try_send(axum::body::Bytes::from(out));
                        }
                    }
                    pos = Some(p + n as u64);
                }
            }
        }
        // Le calage : chaque éclair avec le repère le plus proche, canal par canal.
        while let Some(&at) = flashes.front() {
            if pos.is_some_and(|p| p < at + LIP_SPAN) {
                break;
            }
            flashes.pop_front();
            for (c, e) in edges.iter().enumerate() {
                if let Some(o) = e.offset(at) {
                    per_channel[c] = Some(o);
                    if c == 0 {
                        lip.push_back(o);
                        lip_seen = Some(Instant::now());
                        while lip.len() > 30 {
                            lip.pop_front();
                        }
                    }
                }
            }
        }
        while flashes.len() > 8 {
            flashes.pop_front();
        }
        // Les données (2110-40) : timecode, sous-titres, AFD, SCTE-104.
        if let Some(d) = dr.as_mut() {
            for _ in 0..8 {
                let Some((packets, _)) = d.next_grain() else { break };
                for p in &packets {
                    match (p.did, p.sdid) {
                        (0x60, 0x60) => {
                            if let Some((h, mi, se, fr, df)) = strata_mxl::anc::atc_timecode(p) {
                                tc = Some(format!("{h:02}:{mi:02}:{se:02}{}{fr:02}", if df { ';' } else { ':' }));
                            }
                        }
                        (0x61, 0x01) | (0x61, 0x02) => captions = Some(Instant::now()),
                        (0x41, 0x05) => afd = p.udw.first().map(|b| (b >> 3) & 0x0f),
                        (0x41, 0x07) => scte = Some(jiff::Timestamp::now().strftime("%H:%M:%S").to_string()),
                        _ => {}
                    }
                }
            }
        }
        if snapped.elapsed() >= Duration::from_millis(200) {
            snapped = Instant::now();
            if let Some(m) = meter.as_mut() {
                levels = m.take_levels();
            } else {
                levels.clear();
            }
            // Pour l'historique : la crête de la seconde, la saturation, les canaux muets.
            for l in &levels {
                sec.peak = Some(sec.peak.map_or(l.peak, |p| p.max(l.peak)));
                sec.clip |= l.peak > -0.1;
            }
            if sec.silent.len() != levels.len() {
                sec.silent = vec![true; levels.len()];
            }
            for (i, l) in levels.iter().enumerate() {
                sec.silent[i] &= l.peak < -60.0;
            }
            let format = vr.as_ref().map(|r| crate::streams::engine::format_name(&r.video));
            let frozen = still_since.map(|t| t.elapsed().as_secs_f32()).filter(|t| *t >= 2.0 && !scopes.as_ref().is_some_and(|s| s.black));
            let silent: Vec<bool> = levels.iter().map(|l| l.peak < -60.0).collect();
            let snap = json!({
                "format": format,
                "video": scopes,
                "frozen": frozen,
                "audio": meter.as_ref().map(|m| json!({
                    "levels": levels,
                    "loudness": m.loudness(),
                    "phase": m.phase,
                    "spectrum": m.spectrum(),
                    "gonio": m.gonio(),
                    "silent": silent,
                    "clipping": levels.iter().map(|l| l.peak > -0.1).collect::<Vec<_>>(),
                })),
                "lipsync": {
                    "offset": lip.back(),
                    "history": lip,
                    "channels": per_channel,
                    "detected": lip_seen.is_some_and(|t| t.elapsed() < Duration::from_secs(5)),
                },
                "data": {
                    "timecode": tc,
                    "captions": captions.is_some_and(|t| t.elapsed() < Duration::from_secs(5)),
                    "afd": afd,
                    "scte104": scte,
                },
            });
            *s.snap.lock().unwrap() = snap;
        }
        // L'historique et le journal, chaque seconde.
        if sampled.elapsed() >= Duration::from_secs(1) {
            sampled = Instant::now();
            let t = now_ms();
            let mut h = s.history.lock().unwrap();
            let mut state = 0u32;
            let mut values: Vec<(&'static str, Option<f32>)> = Vec::new();
            // Laisse aux lecteurs le temps de s'ouvrir avant de crier à l'absence.
            let warm = started.elapsed() >= Duration::from_secs(3);
            let format = vr.as_ref().map(|r| crate::streams::engine::format_name(&r.video));
            if let Some(f) = &format {
                if last_format.as_ref().is_some_and(|l| l != f) {
                    h.event(t, Level::Info, "format", json!({ "from": last_format, "to": f }));
                }
                last_format = Some(f.clone());
            }
            if src.video.is_some() {
                let none = scopes.is_none();
                let black = scopes.as_ref().is_some_and(|v| v.black);
                let frozen = !black && still_since.is_some_and(|t| t.elapsed() >= Duration::from_secs(2));
                let gamut = scopes.as_ref().map_or(0.0, |v| v.gamut);
                if warm {
                    h.watch(t, none, Level::Error, "noPicture", "noPictureEnd", json!({}));
                }
                h.watch(t, black, Level::Warn, "black", "blackEnd", json!({}));
                h.watch(t, frozen, Level::Warn, "frozen", "frozenEnd", json!({}));
                h.watch(t, gamut > 1.0, Level::Warn, "gamut", "gamutEnd", json!({ "pct": (gamut * 10.0).round() / 10.0 }));
                state |= if none && warm { history::NO_PICTURE } else { 0 } | if black { history::BLACK } else { 0 } | if frozen { history::FROZEN } else { 0 };
                values.push(("luma", scopes.as_ref().map(|v| v.luma)));
            }
            if !src.audio.is_empty() {
                let none = meter.is_none();
                if warm {
                    h.watch(t, none, Level::Error, "noSound", "noSoundEnd", json!({}));
                }
                let silent: Vec<usize> = sec.silent.iter().enumerate().filter(|(_, x)| **x).map(|(i, _)| i + 1).collect();
                h.watch(t, !silent.is_empty(), Level::Warn, "silence", "silenceEnd", json!({ "channels": silent }));
                h.watch(t, sec.clip, Level::Warn, "clip", "clipEnd", json!({}));
                state |= if none && warm { history::NO_SOUND } else { 0 }
                    | if !silent.is_empty() { history::SILENCE } else { 0 }
                    | if sec.clip { history::CLIP } else { 0 };
                let l = meter.as_ref().map(|m| m.loudness());
                values.push(("momentary", l.as_ref().and_then(|l| l.momentary).map(|x| x as f32)));
                values.push(("shortTerm", l.as_ref().and_then(|l| l.short_term).map(|x| x as f32)));
                values.push(("peak", sec.peak));
            }
            if src.video.is_some() && !src.audio.is_empty() {
                let fresh = lip_seen.is_some_and(|t| t.elapsed() < Duration::from_secs(3));
                let off = lip.back().copied().filter(|_| fresh);
                // Deux mesures de suite hors tolérance (une seule : la mire qui se relance).
                let out = fresh && lip.len() >= 2 && lip.iter().rev().take(2).all(|o| o.abs() > LIP_TOLERANCE_MS);
                h.watch(t, out, Level::Warn, "lipsync", "lipsyncEnd", json!({ "ms": off.map(|o| (o * 10.0).round() / 10.0) }));
                state |= if out { history::LIP } else { 0 };
                values.push(("lip", off.map(|o| o as f32)));
            }
            if !ips.is_empty() {
                let mut lost_s = 0u64;
                let mut down: Vec<String> = Vec::new();
                for f in ips.iter_mut() {
                    let Some(io) = app.st2110.status_of(&f.id) else { continue };
                    if !matches!(io.state, crate::hardware::st2110io::IoState::Running) {
                        down.push(f.name.clone());
                    }
                    let d = f.lost.map_or(0, |p| io.lost.saturating_sub(p));
                    let u = f.unrecovered.map_or(0, |p| io.unrecovered.saturating_sub(p));
                    if u > 0 {
                        h.event(t, Level::Error, "unrecovered", json!({ "flow": f.name, "n": u }));
                    } else if d > 0 {
                        h.event(t, Level::Warn, "loss", json!({ "flow": f.name, "n": d }));
                    }
                    lost_s += d.max(u);
                    f.lost = Some(io.lost);
                    f.unrecovered = Some(io.unrecovered);
                    if Some(&f.id) == video_flow.as_ref() || (video_flow.is_none() && values.iter().all(|(k, _)| *k != "late")) {
                        values.push(("late", io.rtp_late_us.map(|x| (x / 1000.0) as f32)));
                    }
                }
                if warm {
                    h.watch(t, !down.is_empty(), Level::Error, "flowDown", "flowDownEnd", json!({ "flows": down }));
                }
                let ptp = app.clock.read().ptp.clone();
                h.watch(t, !ptp.locked, Level::Error, "ptpUnlocked", "ptpUnlockedEnd", json!({ "state": ptp.port_state }));
                state |= if lost_s > 0 { history::LOSS } else { 0 } | if !ptp.locked { history::PTP } else { 0 };
                values.push(("loss", Some(lost_s as f32)));
                values.push(("ptp", ptp.offset_ns.map(|x| x as f32)));
            }
            h.sample(t / 1000, &values, state);
            sec = Second::default();
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    for l in s.listeners.lock().unwrap().drain(..) {
        drop(l.tx);
    }
}

// ---------- L'API ----------

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/analyze/sources", get(list))
        .route("/api/analyze/state", get(state))
        .route("/api/analyze/reset", post(reset))
        .route("/api/analyze/preview.jpg", get(preview))
        .route("/api/analyze/listen", get(listen))
        .route("/api/analyze/history", get(history_of))
        .route("/api/analyze/watch", post(watch))
        .route("/api/analyze/events/clear", post(clear_events))
}

#[derive(Deserialize)]
struct Pick {
    source: String,
    #[serde(default)]
    solo: String,
    /// Les canaux de la sonie : `1-2`, `1-6`, `3`.
    #[serde(default)]
    loud: String,
    /// La surveillance continue : `1` la met, `0` l'enlève.
    #[serde(default)]
    on: String,
}

/// `1-2` → [0, 1] ; `3` → [2] ; `1,3` → [0, 2].
fn channels_of(s: &str) -> Vec<usize> {
    let mut v = Vec::new();
    for part in s.split(',') {
        let mut it = part.split('-').filter_map(|x| x.trim().parse::<usize>().ok());
        match (it.next(), it.next()) {
            (Some(a), Some(b)) if a >= 1 && b >= a && b <= 64 => v.extend(a - 1..b),
            (Some(a), None) if (1..=64).contains(&a) => v.push(a - 1),
            _ => {}
        }
    }
    v
}

fn find(s: &crate::state::AppState, key: &str) -> Result<Source, (StatusCode, String)> {
    sources(s).into_iter().find(|x| x.key == key).ok_or((StatusCode::NOT_FOUND, "no such source".into()))
}

fn mxl(s: &crate::state::AppState) -> Result<Arc<Mxl>, (StatusCode, String)> {
    crate::hardware::mxl_of(s).ok_or((StatusCode::SERVICE_UNAVAILABLE, "MXL is not available".into()))
}

async fn list(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let s2 = s.clone();
    let v = blocking(move || json!({ "sources": sources(&s2), "mxl": crate::hardware::mxl_of(&s2).is_some(), "watched": s2.analyzer.watched() })).await?;
    Ok(Json(v))
}

async fn state(State(s): State<Shared>, CurrentUser(me): CurrentUser, Query(q): Query<Pick>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let s2 = s.clone();
    let v = blocking(move || {
        let src = find(&s2, &q.source)?;
        let session = s2.analyzer.session(src.clone(), mxl(&s2)?, s2.clone());
        let g = channels_of(&q.loud);
        if !g.is_empty() {
            *session.group.lock().unwrap() = Some(g);
        }
        let mut v = session.snap.lock().unwrap().clone();
        v["source"] = json!(src);
        if src.kind == "2110" || src.kind == "audio" {
            v["ip"] = ip_view(&s2, &src);
        }
        // Le timecode d'une entrée SDI : celui que la capture lit.
        if src.kind == "sdi" && src.way == "in" {
            let tc = s2.captures.list().into_iter().find(|c| format!("sdi/{}/{}", c.card, c.connector) == src.key).and_then(|c| c.timecode);
            if tc.is_some() {
                v["data"]["timecode"] = json!(tc);
            }
        }
        Ok::<_, (StatusCode, String)>(v)
    })
    .await??;
    Ok(Json(v))
}

/// Le détail IP d'une source ST 2110 : chaque flux du groupe (paquets, pertes par réseau,
/// latence de l'émetteur), et l'horloge PTP du serveur.
fn ip_view(s: &crate::state::AppState, src: &Source) -> Value {
    let hw = HardwareConfig::load(&s.data_dir);
    let names: Vec<&String> = src.video.iter().chain(src.audio.iter()).chain(src.data.iter()).collect();
    let flows: Vec<Value> = names
        .iter()
        .filter_map(|n| n.strip_prefix("2110/"))
        .filter_map(|id| hw.flows.iter().find(|f| f.id == id))
        .map(|f| {
            json!({
                "id": f.id,
                "name": f.name,
                "essence": f.essence,
                "red": f.red.to_string(),
                "blue": f.blue.map(|b| b.to_string()),
                "io": s.st2110.status_of(&f.id),
            })
        })
        .collect();
    json!({ "flows": flows, "ptp": s.clock.read().ptp, "redundant": hw.network.media_mode == crate::hardware::MediaMode::Redundant })
}

/// L'historique (10 min, une mesure par seconde) et le journal d'une source.
async fn history_of(State(s): State<Shared>, CurrentUser(me): CurrentUser, Query(q): Query<Pick>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let s2 = s.clone();
    let v = blocking(move || {
        let src = find(&s2, &q.source)?;
        let session = s2.analyzer.session(src, mxl(&s2)?, s2.clone());
        let mut v = session.history.lock().unwrap().view(500);
        v["watched"] = json!(session.pinned.load(Ordering::Relaxed));
        Ok::<_, (StatusCode, String)>(v)
    })
    .await??;
    Ok(Json(v))
}

/// Surveiller une source en continu (son journal continue sans personne devant), ou plus.
async fn watch(State(s): State<Shared>, CurrentUser(me): CurrentUser, Query(q): Query<Pick>) -> ApiResult<StatusCode> {
    require(me.admin)?;
    let s2 = s.clone();
    let user = me.username.clone();
    blocking(move || {
        let src = find(&s2, &q.source)?;
        let on = q.on == "1";
        let session = s2.analyzer.session(src.clone(), mxl(&s2)?, s2.clone());
        session.pinned.store(on, Ordering::Relaxed);
        let level = Level::Info;
        session.history.lock().unwrap().event(now_ms(), level, if on { "watchOn" } else { "watchOff" }, json!({ "user": user }));
        tracing::info!("analyzer: {} {} by {user}", src.label, if on { "watched continuously" } else { "no longer watched" });
        Ok::<_, (StatusCode, String)>(())
    })
    .await??;
    Ok(StatusCode::NO_CONTENT)
}

async fn clear_events(State(s): State<Shared>, CurrentUser(me): CurrentUser, Query(q): Query<Pick>) -> ApiResult<StatusCode> {
    require(me.admin)?;
    let s2 = s.clone();
    blocking(move || {
        let src = find(&s2, &q.source)?;
        s2.analyzer.session(src, mxl(&s2)?, s2.clone()).history.lock().unwrap().clear_events();
        Ok::<_, (StatusCode, String)>(())
    })
    .await??;
    Ok(StatusCode::NO_CONTENT)
}

async fn reset(State(s): State<Shared>, CurrentUser(me): CurrentUser, Query(q): Query<Pick>) -> ApiResult<StatusCode> {
    require(me.admin)?;
    let s2 = s.clone();
    blocking(move || {
        let src = find(&s2, &q.source)?;
        s2.analyzer.session(src, mxl(&s2)?, s2.clone()).reset.store(true, Ordering::Relaxed);
        Ok::<_, (StatusCode, String)>(())
    })
    .await??;
    Ok(StatusCode::NO_CONTENT)
}

async fn preview(State(s): State<Shared>, CurrentUser(me): CurrentUser, Query(q): Query<Pick>) -> ApiResult<impl IntoResponse> {
    require(me.admin)?;
    let s2 = s.clone();
    let jpeg = blocking(move || {
        let src = find(&s2, &q.source).map_err(|e| e.1)?;
        let flow = src.video.ok_or("no picture")?;
        let mxl = crate::hardware::mxl_of(&s2).ok_or("no MXL")?;
        s2.previews.jpeg(&format!("analyze/{flow}"), || crate::hardware::preview::Feed::Mxl { mxl, id: strata_mxl::flow_id(&flow), reader: None })
    })
    .await?
    .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    Ok(([(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "no-store")], (*jpeg).clone()))
}

/// L'écoute : du PCM 16 bits stéréo à 48 kHz, en continu. `solo` : les canaux écoutés seuls
/// (`1,3`) ; vide : tous.
async fn listen(State(s): State<Shared>, CurrentUser(me): CurrentUser, Query(q): Query<Pick>) -> ApiResult<impl IntoResponse> {
    require(me.admin)?;
    let solo: Vec<usize> = q.solo.split(',').filter_map(|x| x.trim().parse::<usize>().ok()).filter(|c| (1..=64).contains(c)).map(|c| c - 1).collect();
    let (tx, rx) = tokio::sync::mpsc::channel::<axum::body::Bytes>(64);
    let s2 = s.clone();
    blocking(move || {
        let src = find(&s2, &q.source)?;
        let session = s2.analyzer.session(src, mxl(&s2)?, s2.clone());
        session.listeners.lock().unwrap().push(Listener { solo, tx });
        Ok::<_, (StatusCode, String)>(())
    })
    .await??;
    let body = futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|b| (Ok::<_, std::io::Error>(b), rx)) });
    Ok(([(header::CONTENT_TYPE, "audio/L16; rate=48000; channels=2"), (header::CACHE_CONTROL, "no-store")], axum::body::Body::from_stream(body)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_lists() {
        assert_eq!(channels_of("1-2"), [0, 1]);
        assert_eq!(channels_of("3"), [2]);
        assert_eq!(channels_of("1-6"), [0, 1, 2, 3, 4, 5]);
        assert_eq!(channels_of("1,4"), [0, 3]);
        assert!(channels_of("x").is_empty() && channels_of("0").is_empty());
    }

    #[test]
    fn the_mix_follows_the_solos() {
        let f = [0.1, 0.2, 0.3, 0.4];
        let (l, r) = listen_mix(&f, &[]);
        let g = 1.0 / 2f32.sqrt();
        assert!((l - 0.4 * g).abs() < 1e-6 && (r - 0.6 * g).abs() < 1e-6);
        assert_eq!(listen_mix(&f, &[2]), (0.3, 0.3));
        let (l, r) = listen_mix(&f, &[0, 3]);
        assert!((l - 0.1).abs() < 1e-6 && (r - 0.4).abs() < 1e-6);
        assert_eq!(listen_mix(&f, &[9]), (0.0, 0.0));
    }
}
