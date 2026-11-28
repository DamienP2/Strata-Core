//! Santé › Mires et analyse, l'analyseur : n'importe quelle source du serveur (entrée SDI, flux
//! ST 2110 reçu avec ou sans image, flux réseau reçu, et ce qui part : sorties, destinations,
//! mires) analysée par le Core pendant qu'on la regarde — scopes, son, sonie R128, calage
//! image/son, données — et écoutée dans le navigateur (tous les canaux, ou ceux en solo).
//!
//! - Une session par source, lancée à la première demande, arrêtée 15 s après la dernière (et
//!   sans personne qui écoute).
//! - L'écoute : du PCM 16 bits stéréo 48 kHz en flux HTTP continu (le navigateur le joue).
pub mod audio;
pub mod video;

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
    listeners: Mutex<Vec<Listener>>,
    done: AtomicBool,
}

#[derive(Default)]
pub struct Analyzer {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

impl Analyzer {
    fn session(&self, src: Source, mxl: Arc<Mxl>) -> Arc<Session> {
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
            listeners: Mutex::new(Vec::new()),
            done: AtomicBool::new(false),
        });
        let s2 = s.clone();
        let _ = std::thread::Builder::new().name(format!("analyze-{}", src.key)).spawn(move || {
            run(&s2, &mxl);
            s2.done.store(true, Ordering::Relaxed);
        });
        map.insert(src.key, s.clone());
        s
    }

    pub fn stop_all(&self) {
        for s in self.sessions.lock().unwrap().drain().map(|(_, s)| s) {
            s.stop.store(true, Ordering::Relaxed);
        }
    }
}

/// Une mesure du calage : l'éclair (index de son de son image) et le début du silence.
const LIP_SPAN: u64 = 4_800;

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

fn run(s: &Session, mxl: &Mxl) {
    let src = &s.source;
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
    let mut onsets: VecDeque<u64> = VecDeque::new();
    let (mut zero_run, mut zero_start, mut tone) = (0u64, 0u64, false);
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
        if idle && !listening {
            break;
        }
        // Les flux, revus chaque seconde.
        if checked.elapsed() >= Duration::from_secs(1) {
            checked = Instant::now();
            if let Some(id) = vid {
                if !mxl.is_active(id) {
                    vr = None;
                } else if vr.is_none() {
                    vr = mxl.video_reader(id).ok();
                }
            }
            for (i, id) in aids.iter().enumerate() {
                if !mxl.is_active(*id) {
                    ars[i] = None;
                } else if ars[i].is_none() {
                    ars[i] = mxl.audio_reader(*id).ok();
                }
            }
            if let Some(id) = did {
                if !mxl.is_active(id) {
                    dr = None;
                } else if dr.is_none() {
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
        if s.reset.swap(false, Ordering::Relaxed) {
            if let Some(m) = meter.as_mut() {
                m.reset();
            }
            lip.clear();
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
                        // Le début du silence du canal 1 (le bip coupé de la mire de calage).
                        for k in 0..n {
                            if chunk[k * total].abs() < 1e-6 {
                                if zero_run == 0 {
                                    zero_start = p + k as u64;
                                }
                                zero_run += 1;
                                if zero_run == 24 && tone {
                                    onsets.push_back(zero_start);
                                    tone = false;
                                }
                            } else {
                                zero_run = 0;
                                tone = true;
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
        // Le calage : chaque éclair avec le silence le plus proche (±100 ms).
        while let Some(&at) = flashes.front() {
            if pos.is_some_and(|p| p < at + LIP_SPAN) {
                break;
            }
            flashes.pop_front();
            if let Some(o) = onsets.iter().copied().filter(|o| o.abs_diff(at) <= LIP_SPAN).min_by_key(|o| o.abs_diff(at)) {
                lip.push_back((o as f64 - at as f64) / 48.0);
                lip_seen = Some(Instant::now());
                while lip.len() > 30 {
                    lip.pop_front();
                }
            }
        }
        while onsets.len() > 8 {
            onsets.pop_front();
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
                    "silent": silent,
                    "clipping": levels.iter().map(|l| l.peak > -0.1).collect::<Vec<_>>(),
                })),
                "lipsync": {
                    "offset": lip.back(),
                    "history": lip,
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
}

#[derive(Deserialize)]
struct Pick {
    source: String,
    #[serde(default)]
    solo: String,
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
    let v = blocking(move || json!({ "sources": sources(&s2), "mxl": crate::hardware::mxl_of(&s2).is_some() })).await?;
    Ok(Json(v))
}

async fn state(State(s): State<Shared>, CurrentUser(me): CurrentUser, Query(q): Query<Pick>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let s2 = s.clone();
    let v = blocking(move || {
        let src = find(&s2, &q.source)?;
        let session = s2.analyzer.session(src.clone(), mxl(&s2)?);
        let mut v = session.snap.lock().unwrap().clone();
        v["source"] = json!(src);
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

async fn reset(State(s): State<Shared>, CurrentUser(me): CurrentUser, Query(q): Query<Pick>) -> ApiResult<StatusCode> {
    require(me.admin)?;
    let s2 = s.clone();
    blocking(move || {
        let src = find(&s2, &q.source)?;
        s2.analyzer.session(src, mxl(&s2)?).reset.store(true, Ordering::Relaxed);
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
        let session = s2.analyzer.session(src, mxl(&s2)?);
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
