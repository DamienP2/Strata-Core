//! Le moteur des flux réseau : un fil par entrée, un par sortie et un par destination, chacun avec
//! son ffmpeg, relancé quand il tombe (attente croissante, 30 s au plus).
//!
//! - **Entrée** : ffmpeg décode ; l'image arrive en v210 sur stdout, le son en float32 sur le
//!   fd 3 ; le Core les écrit en MXL. Pas de son dans la source : du silence (le flux audio
//!   existe toujours pour les applis).
//! - **Sortie** : le Core lit le flux MXL à la cadence de la sortie (l'image d'avant pour un
//!   raté, du noir après une seconde sans rien) et le passe à un ffmpeg qui encode une fois, en
//!   MPEG-TS sur stdout ; le Core recopie ce TS à chaque destination, qui a son propre ffmpeg
//!   (sans réencoder) : une destination qui tombe est relancée seule.
use serde::Serialize;
use std::{
    collections::HashMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use strata_mxl::{Mxl, Video};

use super::{audio_of, ffmpeg, input_flow, output_flow, scheme, Destination, InputCfg, OutputCfg, StreamsConfig};

// ---------- L'état ----------

#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum State {
    #[default]
    Starting,
    /// Ça passe.
    Running,
    /// Entrée : on attend la source (ou qu'on la pousse). Sortie : rien à envoyer.
    Waiting,
    Error,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct InputStatus {
    pub state: State,
    /// Ce que la source envoie (« Video: h264 (High), 1920x1080, 25 fps »).
    pub source: Vec<String>,
    /// Ce qui est écrit en MXL (« 1080i50 »).
    pub format: Option<String>,
    pub frames: u64,
    pub fps: f64,
    pub audio: bool,
    /// Qui décode : `nvdec`, `vaapi` (le GPU) ou `cpu`.
    pub decoder: Option<String>,
    pub restarts: u32,
    pub error: Option<String>,
    /// Depuis quand (ms) dans cet état.
    pub since: u64,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct DestStatus {
    pub id: String,
    pub state: State,
    pub kbps: f64,
    pub restarts: u32,
    pub error: Option<String>,
    pub since: u64,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct OutputStatus {
    pub state: State,
    /// `nvenc`, `vaapi`, `x264`.
    pub encoder: Option<String>,
    /// Ce qui est envoyé (« 1080p50 »).
    pub format: Option<String>,
    /// La source arrive (sinon : du noir et du silence).
    pub live: bool,
    pub frames: u64,
    pub fps: f64,
    pub kbps: f64,
    /// Images de son manquantes (du silence à la place) depuis le départ.
    pub audio_missing: u64,
    pub error: Option<String>,
    pub destinations: Vec<DestStatus>,
    pub since: u64,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn set_state<T>(s: &mut T, state: State, get: impl Fn(&mut T) -> (&mut State, &mut u64)) {
    let (st, since) = get(s);
    if *st != state {
        *st = state;
        *since = now_ms();
    }
}

/// Le nom d'une vidéo (« 1080i50 », « 720p59.94 »).
pub fn format_name(v: &Video) -> String {
    let r = v.rate.0 as f64 / v.rate.1.max(1) as f64 * if v.interlaced { 2.0 } else { 1.0 };
    let r = (r * 100.0).round() / 100.0;
    format!("{}{}{}", v.height, if v.interlaced { 'i' } else { 'p' }, if r.fract() == 0.0 { format!("{r:.0}") } else { format!("{r}") })
}

fn video_of(name: &str) -> Option<Video> {
    crate::hardware::parse_format(name).map(|(width, height, rate, interlaced)| Video { width, height, rate, interlaced })
}

// ---------- Le moteur ----------

/// Ce qui ne dépend pas d'un flux : MXL, le format maison, où ranger le HLS, l'encodeur du GPU.
#[derive(Clone)]
pub struct Env {
    pub mxl: Arc<Mxl>,
    pub house: Video,
    pub hls_dir: PathBuf,
    /// L'encodeur matériel possible : `nvenc`, ou `vaapi` avec son nœud de rendu.
    pub gpu: Option<(String, Option<String>)>,
}

struct Running<K> {
    key: K,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl<K> Running<K> {
    fn halt(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[derive(Default)]
pub struct Engine {
    inputs: Mutex<HashMap<String, Running<(InputCfg, Video)>>>,
    outputs: Mutex<HashMap<String, Running<(OutputCfg, Video)>>>,
    in_status: Arc<Mutex<HashMap<String, InputStatus>>>,
    out_status: Arc<Mutex<HashMap<String, OutputStatus>>>,
    /// Pourquoi rien ne tourne (ffmpeg ou MXL absent).
    problem: Mutex<Option<String>>,
    ffmpeg: std::sync::OnceLock<Result<String, String>>,
}

impl Engine {
    /// La version de ffmpeg, ou pourquoi il manque.
    pub fn ffmpeg(&self) -> Result<String, String> {
        self.ffmpeg.get_or_init(ffmpeg::version).clone()
    }

    /// Tient les flux en accord avec les réglages (bloquant).
    pub fn reconcile(&self, cfg: &StreamsConfig, env: Option<Env>) {
        let any = cfg.inputs.iter().any(|i| i.enabled) || cfg.outputs.iter().any(|o| o.enabled);
        let problem = if !any {
            None
        } else if let Err(e) = self.ffmpeg() {
            Some(format!("ffmpeg is missing ({e}): install it (sudo apt install ffmpeg)"))
        } else if env.is_none() {
            Some("the network streams need MXL (the pictures are shared with the apps through it)".to_string())
        } else {
            None
        };
        let env = match (env, &problem) {
            (Some(e), None) => Some(e),
            _ => None,
        };
        *self.problem.lock().unwrap() = problem;
        // Les entrées.
        let wanted: Vec<(InputCfg, Video)> = match &env {
            Some(e) => cfg.inputs.iter().filter(|i| i.enabled).map(|i| (i.clone(), e.house)).collect(),
            None => Vec::new(),
        };
        let mut stopped = Vec::new();
        {
            let mut run = self.inputs.lock().unwrap();
            run.retain(|id, r| {
                let keep = wanted.iter().any(|w| &w.0.id == id && *w == r.key);
                if !keep {
                    stopped.push(std::mem::replace(r, Running { key: r.key.clone(), stop: Arc::new(AtomicBool::new(true)), thread: None }));
                }
                keep
            });
            for w in wanted {
                if run.contains_key(&w.0.id) {
                    continue;
                }
                let env = env.clone().unwrap();
                let stop = Arc::new(AtomicBool::new(false));
                let status = self.in_status.clone();
                let (c, s) = (w.0.clone(), stop.clone());
                tracing::info!("network input {} ({})", c.name, c.url);
                let thread = std::thread::Builder::new().name(format!("in-{}", c.id)).spawn(move || run_input(&c, &env, &s, &status)).ok();
                run.insert(w.0.id.clone(), Running { key: w, stop, thread });
            }
        }
        // Les sorties : au moins une destination active.
        let wanted: Vec<(OutputCfg, Video)> = match &env {
            Some(e) => cfg.outputs.iter().filter(|o| o.enabled && o.destinations.iter().any(|d| d.enabled)).map(|o| (o.clone(), e.house)).collect(),
            None => Vec::new(),
        };
        let mut stopped_out = Vec::new();
        {
            let mut run = self.outputs.lock().unwrap();
            run.retain(|id, r| {
                let keep = wanted.iter().any(|w| &w.0.id == id && *w == r.key);
                if !keep {
                    stopped_out.push(std::mem::replace(r, Running { key: r.key.clone(), stop: Arc::new(AtomicBool::new(true)), thread: None }));
                }
                keep
            });
            for w in wanted {
                if run.contains_key(&w.0.id) {
                    continue;
                }
                let env = env.clone().unwrap();
                let stop = Arc::new(AtomicBool::new(false));
                let status = self.out_status.clone();
                let (c, s) = (w.0.clone(), stop.clone());
                tracing::info!("network output {} ({} destinations)", c.name, c.destinations.iter().filter(|d| d.enabled).count());
                let thread = std::thread::Builder::new().name(format!("out-{}", c.id)).spawn(move || run_output(&c, &env, &s, &status)).ok();
                run.insert(w.0.id.clone(), Running { key: w, stop, thread });
            }
        }
        for r in stopped {
            self.in_status.lock().unwrap().remove(&r.key.0.id);
            tracing::info!("network input {} stopped", r.key.0.name);
            r.halt();
        }
        for r in stopped_out {
            self.out_status.lock().unwrap().remove(&r.key.0.id);
            tracing::info!("network output {} stopped", r.key.0.name);
            r.halt();
        }
    }

    pub fn stop_all(&self) {
        self.reconcile(&StreamsConfig::default(), None);
    }

    pub fn input_status(&self, id: &str) -> Option<InputStatus> {
        self.in_status.lock().unwrap().get(id).cloned()
    }

    pub fn output_status(&self, id: &str) -> Option<OutputStatus> {
        self.out_status.lock().unwrap().get(id).cloned()
    }

    pub fn problem(&self) -> Option<String> {
        self.problem.lock().unwrap().clone()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop_all();
    }
}

/// Attend `d`, ou moins si on arrête.
fn pause(stop: &AtomicBool, d: Duration) {
    let end = Instant::now() + d;
    while Instant::now() < end && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn backoff(n: u32) -> Duration {
    Duration::from_secs((1u64 << n.min(5)).min(30))
}

// ---------- Les entrées ----------

/// Les options de ffmpeg pour une entrée : la source, l'image (v210 sur stdout), le son
/// (float32 sur le fd 3), le relais local.
/// La vidéo à écrire en MXL pour une source (« Video: h264 (High), yuv420p(tv, bt709, top first),
/// 1920x1080, 25 fps… ») : en 1080, à sa cadence, entrelacée si elle l'est. None : cadence
/// inconnue (on garde le format maison).
pub fn source_video(desc: &str) -> Option<Video> {
    let interlaced = desc.contains("top first") || desc.contains("bottom first");
    let fps: f64 = desc.split(", ").find_map(|p| p.strip_suffix(" fps")).and_then(|x| x.trim().parse().ok())?;
    // La cadence d'images la plus proche parmi les cadences de diffusion.
    let rates = [23.976, 24.0, 25.0, 29.97, 30.0, 50.0, 59.94, 60.0];
    let r = rates.iter().copied().min_by(|a, b| (a - fps).abs().total_cmp(&(b - fps).abs()))?;
    if (r - fps).abs() > 0.5 {
        return None;
    }
    let name = |x: f64| if x.fract() == 0.0 { format!("{x:.0}") } else { format!("{x}") };
    // En entrelacé, ffmpeg donne les images par seconde (25 pour du 1080i50).
    let f = if interlaced {
        format!(
            "1080i{}",
            name(if r == 23.976 {
                47.952
            } else if r == 29.97 {
                59.94
            } else {
                r * 2.0
            })
        )
    } else {
        format!("1080p{}", name(r))
    };
    video_of(&f)
}

/// Le décodage par le GPU : `cuda` (NVIDIA) ou `vaapi` et son nœud de rendu.
pub type HwDecode = (String, Option<String>);

/// Le décodage par le GPU d'après l'encodeur qu'il offre (le même GPU décode).
fn hw_decode(env: &Env) -> Option<HwDecode> {
    match &env.gpu {
        Some((e, _)) if e == "nvenc" => Some(("cuda".into(), None)),
        Some((e, node)) if e == "vaapi" => Some(("vaapi".into(), node.clone())),
        _ => None,
    }
}

pub fn input_args(c: &InputCfg, v: &Video, audio: bool, hw: Option<&HwDecode>) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let sch = scheme(&c.url).unwrap_or("");
    match sch {
        "rtsp" | "rtsps" => a.extend(["-rtsp_transport", "tcp"].map(String::from)),
        // Sur 0.0.0.0 : le Core attend qu'on lui pousse le flux.
        "rtmp" | "rtmps" if c.url.split("://").nth(1).is_some_and(|h| h.starts_with("0.0.0.0") || h.starts_with('@')) => {
            a.extend(["-listen", "1"].map(String::from));
        }
        "rtmp" | "rtmps" => a.extend(["-rw_timeout", "10000000"].map(String::from)),
        // HLS ou fichier : lu à son rythme, pas plus vite.
        "http" | "https" => a.extend(["-re", "-rw_timeout", "10000000"].map(String::from)),
        _ => {}
    }
    let url = if sch == "rtmp" { c.url.replacen("rtmp://@", "rtmp://0.0.0.0", 1) } else { c.url.clone() };
    // Décodé par le GPU quand il y en a un (les images reviennent en mémoire pour la suite).
    match hw {
        Some((kind, node)) if kind == "vaapi" => {
            a.extend(["-hwaccel".into(), "vaapi".into(), "-hwaccel_device".into(), node.clone().unwrap_or_else(|| "/dev/dri/renderD128".into())]);
        }
        Some((kind, _)) => a.extend(["-hwaccel".into(), kind.clone()]),
        None => {}
    }
    a.extend(["-fflags", "+genpts+discardcorrupt", "-i"].map(String::from));
    a.push(url);
    // L'image : à la cadence et à la taille voulues, en v210.
    let rate = format!("{}/{}", v.rate.0, v.rate.1.max(1));
    let vf = if v.interlaced {
        format!("fps={rate},scale={}:{}:interl=-1,setfield=tff,setsar=1", v.width, v.height)
    } else {
        format!("yadif=mode=send_frame:deint=interlaced,fps={rate},scale={}:{},setsar=1", v.width, v.height)
    };
    a.extend(["-map", "0:v:0", "-vf"].map(String::from));
    a.push(vf);
    a.extend(["-c:v", "v210", "-f", "rawvideo", "pipe:1"].map(String::from));
    if audio {
        a.extend(
            ["-map", "0:a:0?", "-ac", "2", "-ar", "48000", "-af", "aresample=async=1000", "-c:a", "pcm_f32le", "-flush_packets", "1", "-f", "f32le", "pipe:3"]
                .map(String::from),
        );
    }
    if c.relay && c.relay_port != 0 {
        a.extend(["-map", "0:v?", "-map", "0:a?", "-c", "copy", "-f", "mpegts"].map(String::from));
        a.push(format!("udp://127.0.0.1:{}?pkt_size=1316", c.relay_port));
    }
    a
}

fn run_input(c: &InputCfg, env: &Env, stop: &AtomicBool, status: &Mutex<HashMap<String, InputStatus>>) {
    let set = |f: &dyn Fn(&mut InputStatus)| f(status.lock().unwrap().entry(c.id.clone()).or_default());
    // « auto » (vide) : la cadence et le balayage de la source, en 1080, dès qu'ils sont connus
    // (le format maison en attendant) ; « house » : le format maison ; sinon le format demandé.
    let auto = c.format.trim().is_empty();
    let mut v = match c.format.trim() {
        "" | "house" => env.house,
        f => video_of(f).unwrap_or(env.house),
    };
    set(&|s| s.format = Some(format_name(&v)));
    let vid = strata_mxl::flow_id(&input_flow(&c.id));
    let aid = strata_mxl::flow_id(&audio_of(&input_flow(&c.id)));
    let writers = env.mxl.video_writer(vid, &c.name, &v).and_then(|w| Ok((w, env.mxl.audio_writer(aid, &c.name, 2)?)));
    let (mut vw, mut aw) = match writers {
        Ok((v, a)) => (Some(v), a),
        Err(e) => {
            set(&|s| {
                set_state(s, State::Error, |s| (&mut s.state, &mut s.since));
                s.error = Some(e.clone());
            });
            return;
        }
    };
    let mut audio = true;
    let mut hw = hw_decode(env);
    let mut failures = 0u32;
    while !stop.load(Ordering::Relaxed) {
        set(&|s| {
            if s.state != State::Error {
                set_state(s, State::Starting, |s| (&mut s.state, &mut s.since));
            }
        });
        let decoder = match hw.as_ref().map(|h| h.0.as_str()) {
            Some("cuda") => "nvdec",
            Some(k) => k,
            None => "cpu",
        }
        .to_string();
        set(&|s| {
            s.decoder = Some(decoder.clone());
            s.format = Some(format_name(&v));
        });
        let stride = strata_mxl::v210_stride(v.width);
        let frame_bytes = stride * v.height as usize;
        let mut p = match ffmpeg::spawn(&input_args(c, &v, audio, hw.as_ref()), false, true, if audio { ffmpeg::Fd3::Read } else { ffmpeg::Fd3::None }) {
            Ok(p) => p,
            Err(e) => {
                set(&|s| {
                    set_state(s, State::Error, |s| (&mut s.state, &mut s.since));
                    s.error = Some(e.clone());
                });
                pause(stop, Duration::from_secs(10));
                continue;
            }
        };
        // Le son : lu à part, écrit en MXL tel qu'il vient.
        // Le son : mis de côté tel qu'il vient (une source TS le livre par paquets de plusieurs
        // centaines de ms), puis écrit en MXL au rythme des images, une image de son à la fois :
        // il reste calé sur l'image.
        let heard = Arc::new(AtomicBool::new(false));
        let queue: Arc<Mutex<std::collections::VecDeque<f32>>> = Arc::new(Mutex::new(std::collections::VecDeque::new()));
        let audio_thread = p.fd3.take().map(|mut f| {
            let (heard, queue) = (heard.clone(), queue.clone());
            std::thread::spawn(move || {
                let mut buf = vec![0u8; 16384];
                let mut carry: Vec<u8> = Vec::with_capacity(8);
                loop {
                    match f.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            heard.store(true, Ordering::Relaxed);
                            carry.extend_from_slice(&buf[..n]);
                            let whole = carry.len() - carry.len() % 4;
                            let mut q = queue.lock().unwrap();
                            q.extend(carry[..whole].as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)));
                            // Plus de 2 s d'avance : le plus ancien est jeté.
                            let max = 2 * 48_000 * 2;
                            if q.len() > max {
                                let extra = q.len() - max;
                                q.drain(..extra);
                            }
                            carry.drain(..whole);
                        }
                    }
                }
            })
        });
        // L'image : lue par un fil, pour pouvoir arrêter ou constater un silence.
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(2);
        let mut out = p.child.stdout.take();
        let reader = std::thread::spawn(move || {
            let Some(out) = out.as_mut() else { return };
            loop {
                let mut frame = vec![0u8; frame_bytes];
                if out.read_exact(&mut frame).is_err() || tx.send(frame).is_err() {
                    break;
                }
            }
        });
        let started = Instant::now();
        let mut reformat = false;
        let (mut frames, mut window, mut window_start) = (0u64, 0u64, Instant::now());
        let mut last_frame = Instant::now();
        let mut acc = 0u64;
        let _ = started;
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(frame) => {
                    // Le son de cette image (du silence s'il n'y en a pas, ou pas encore).
                    acc += 48_000 * v.rate.1.max(1) as u64;
                    let n = (acc / v.rate.0.max(1) as u64) as usize;
                    acc %= v.rate.0.max(1) as u64;
                    let mut pcm: Vec<f32> = {
                        let mut q = queue.lock().unwrap();
                        let take = (n * 2).min(q.len());
                        q.drain(..take).collect()
                    };
                    // Une image qui arrive trop en avance (rattrapage après un à-coup de la source)
                    // est jetée : MXL ne garde que 200 ms, on reste près de maintenant. Son son est
                    // gardé, posé à la suite du précédent.
                    let Some(vw) = vw.as_mut() else { continue };
                    if vw.ahead() >= if v.interlaced { 4 } else { 2 } {
                        pcm.resize(n * 2, 0.0);
                        let _ = aw.write_interleaved(&pcm);
                        continue;
                    }
                    let _ = vw.write_v210(&frame, stride);
                    pcm.resize(n * 2, 0.0);
                    let _ = match vw.last_frame_audio_index() {
                        Some(at) => aw.write_at(at, &pcm),
                        None => aw.write_interleaved(&pcm),
                    };
                    frames += 1;
                    window += 1;
                    last_frame = Instant::now();
                    if frames == 1 {
                        failures = 0;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            // Plus rien depuis 10 s (source figée) : on relance.
            if frames > 0 && last_frame.elapsed() > Duration::from_secs(10) {
                set(&|s| s.error = Some("no picture for 10 s".into()));
                break;
            }
            if window_start.elapsed() >= Duration::from_secs(1) {
                let fps = window as f64 / window_start.elapsed().as_secs_f64();
                let log = p.log.lock().unwrap().clone();
                // En « auto » : la source est connue ; une autre cadence ou un autre balayage que
                // ce qui s'écrit fait relancer ffmpeg au bon format (sans désentrelacer pour rien).
                if auto && !reformat {
                    if let Some(want) = log.streams.iter().find(|d| d.starts_with("Video:")).and_then(|d| source_video(d)) {
                        if want != v {
                            tracing::info!("network input {}: the source is {}, written as such", c.name, format_name(&want));
                            v = want;
                            reformat = true;
                            break;
                        }
                    }
                }
                let has_audio = audio && heard.load(Ordering::Relaxed);
                set(&|s| {
                    s.frames = frames;
                    s.fps = (fps * 10.0).round() / 10.0;
                    s.source = log.streams.clone();
                    s.audio = has_audio;
                    if frames > 0 && last_frame.elapsed() < Duration::from_secs(2) {
                        set_state(s, State::Running, |s| (&mut s.state, &mut s.since));
                        s.error = None;
                    } else if frames == 0 {
                        // On attend la source (un émetteur SRT/RTMP qui se connecte).
                        set_state(s, State::Waiting, |s| (&mut s.state, &mut s.since));
                    }
                });
                window = 0;
                window_start = Instant::now();
            }
        }
        p.stop();
        let _ = reader.join();
        if let Some(t) = audio_thread {
            let _ = t.join();
        }
        if stop.load(Ordering::Relaxed) {
            break;
        }
        // Le format de la source vient d'être connu : le flux MXL est recréé à ce format.
        if reformat {
            // L'ancien écrivain lâché d'abord : MXL recrée alors le flux au nouveau format.
            drop(vw.take());
            match env.mxl.video_writer(vid, &c.name, &v) {
                Ok(w) => vw = Some(w),
                Err(e) => tracing::warn!("network input {}: {e}", c.name),
            }
            continue;
        }
        let log = p.log.lock().unwrap().clone();
        let errors = log.errors.join(" ");
        // Une source sans son : relancée sans sortie audio (du silence à la place).
        // Le GPU ne décode pas ce flux (pilote, codec) : décodé par le processeur.
        if hw.is_some() && (errors.contains("Hardware device setup failed") || errors.contains("Device creation failed") || errors.contains("hwaccel")) {
            tracing::info!("network input {}: GPU decoding unavailable, decoding on the CPU", c.name);
            hw = None;
            continue;
        }
        if audio && (errors.contains("does not contain any stream") || errors.contains("matches no streams")) {
            audio = false;
            continue;
        }
        failures += 1;
        let why = log.last_error().unwrap_or_else(|| "the source stopped".into());
        set(&|s| {
            set_state(s, State::Error, |s| (&mut s.state, &mut s.since));
            s.error = Some(why.clone());
            s.restarts += 1;
        });
        pause(stop, backoff(failures));
    }
}

// ---------- Les sorties ----------

/// L'encodeur d'une sortie : celui demandé, ou en `auto` le GPU s'il y en a un.
fn encoder_of(c: &OutputCfg, env: &Env) -> (String, Option<String>) {
    match (c.encoder.as_str(), &env.gpu) {
        ("x264", _) => ("x264".into(), None),
        ("auto", Some((e, node))) => (e.clone(), node.clone()),
        ("nvenc", _) => ("nvenc".into(), None),
        ("vaapi", Some((e, node))) if e == "vaapi" => ("vaapi".into(), node.clone()),
        ("vaapi", _) => ("vaapi".into(), Some("/dev/dri/renderD128".into())),
        _ => ("x264".into(), None),
    }
}

/// Les options de l'encodeur : v210 sur stdin et float32 sur le fd 3 (à la cadence `feed`), un
/// MPEG-TS sur stdout (`target` : ce qui part).
pub fn encoder_args(c: &OutputCfg, feed: &Video, target: &Video, encoder: &str, node: Option<&str>) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    if encoder == "vaapi" {
        a.extend(["-vaapi_device".into(), node.unwrap_or("/dev/dri/renderD128").to_string()]);
    }
    // L'image telle que MXL la garde (v210, 10 bits) : ffmpeg la lit sans que le Core la convertisse.
    a.extend(["-f", "v210", "-video_size"].map(String::from));
    a.push(format!("{}x{}", feed.width, feed.height));
    a.push("-framerate".into());
    a.push(format!("{}/{}", feed.rate.0, feed.rate.1.max(1)));
    a.extend(["-thread_queue_size", "64", "-i", "pipe:0"].map(String::from));
    a.extend(["-f", "f32le", "-ar", "48000", "-ac", "2", "-thread_queue_size", "512", "-i", "pipe:3", "-map", "0:v", "-map", "1:a"].map(String::from));
    let rate = format!("{}/{}", target.rate.0, target.rate.1.max(1));
    // Le v210 lu (10 bits) passe tout de suite en 8 bits 4:2:2 : le désentrelacement et la mise à
    // l'échelle travaillent en 8 bits, comme l'encodeur (H.264 8 bits).
    let mut vf = vec!["format=yuv422p".to_string()];
    if feed.interlaced {
        vf.push("setfield=tff".to_string());
        vf.push("bwdif=mode=send_field".to_string());
    }
    vf.push(format!("fps={rate}"));
    if (feed.width, feed.height) != (target.width, target.height) {
        vf.push(format!("scale={}:{}", target.width, target.height));
    }
    let gop = (2 * target.rate.0 / target.rate.1.max(1)).max(1).to_string();
    let (v, buf) = (format!("{}k", c.video_kbps), format!("{}k", c.video_kbps * 2));
    match encoder {
        "nvenc" => {
            vf.push("format=yuv420p".into());
            a.extend(["-vf".into(), vf.join(",")]);
            a.extend(["-c:v", "h264_nvenc", "-preset", "p4", "-tune", "ll", "-rc", "cbr", "-profile:v", "high"].map(String::from));
        }
        "vaapi" => {
            vf.push("format=nv12".into());
            vf.push("hwupload".into());
            a.extend(["-vf".into(), vf.join(",")]);
            a.extend(["-c:v", "h264_vaapi", "-rc_mode", "CBR", "-profile:v", "high"].map(String::from));
        }
        _ => {
            vf.push("format=yuv420p".into());
            a.extend(["-vf".into(), vf.join(",")]);
            a.extend(["-c:v", "libx264", "-preset", "veryfast", "-tune", "zerolatency", "-profile:v", "high", "-sc_threshold", "0"].map(String::from));
        }
    }
    a.extend(["-b:v".into(), v.clone(), "-maxrate".into(), v, "-bufsize".into(), buf, "-g".into(), gop.clone(), "-keyint_min".into(), gop]);
    // Les en-têtes avant chaque image clé : une destination peut prendre le flux en route.
    a.extend(["-bsf:v", "dump_extra=freq=keyframe"].map(String::from));
    a.extend(["-c:a".into(), "aac".into(), "-b:a".into(), format!("{}k", c.audio_kbps), "-ar".into(), "48000".into(), "-ac".into(), "2".into()]);
    a.extend(["-f", "mpegts", "-muxdelay", "0", "-flush_packets", "1", "pipe:1"].map(String::from));
    a
}

/// L'adresse d'une destination, sa clé comprise.
pub fn destination_url(d: &Destination) -> String {
    let key = d.key.trim();
    match scheme(&d.url) {
        Some("rtmp" | "rtmps") if !key.is_empty() => format!("{}/{}", d.url.trim_end_matches('/'), key),
        Some("srt") => {
            let mut u = d.url.clone();
            let mut add = |k: &str, v: &str| {
                if !u.contains(&format!("{k}=")) {
                    u.push(if u.contains('?') { '&' } else { '?' });
                    u.push_str(&format!("{k}={v}"));
                }
            };
            add("mode", "caller");
            add("pkt_size", "1316");
            if !key.is_empty() {
                add("streamid", key);
            }
            u
        }
        Some("udp") if !d.url.contains("pkt_size") => format!("{}{}pkt_size=1316", d.url, if d.url.contains('?') { '&' } else { '?' }),
        _ => d.url.clone(),
    }
}

/// Les options d'une destination : le TS de l'encodeur sur stdin, recopié sans réencoder.
pub fn destination_args(d: &Destination, hls_dir: &Path) -> Vec<String> {
    let mut a: Vec<String> =
        ["-f", "mpegts", "-analyzeduration", "2000000", "-probesize", "2000000", "-i", "pipe:0", "-map", "0", "-c", "copy"].map(String::from).to_vec();
    match (d.url.as_str(), scheme(&d.url)) {
        ("hls", _) => {
            a.extend(
                [
                    "-f",
                    "hls",
                    "-hls_time",
                    "2",
                    "-hls_list_size",
                    "6",
                    "-hls_flags",
                    "delete_segments+independent_segments+omit_endlist",
                    "-hls_segment_filename",
                ]
                .map(String::from),
            );
            a.push(hls_dir.join("seg%05d.ts").display().to_string());
            a.push(hls_dir.join("index.m3u8").display().to_string());
            return a;
        }
        (_, Some("rtmp" | "rtmps")) => a.extend(["-f", "flv", "-flvflags", "no_duration_filesize"].map(String::from)),
        (_, Some("rtp")) => a.extend(["-f", "rtp_mpegts"].map(String::from)),
        _ => a.extend(["-f", "mpegts"].map(String::from)),
    }
    a.push(destination_url(d));
    a
}

/// La source d'une sortie : son flux vidéo et son flux audio.
fn source_of(c: &OutputCfg) -> String {
    if c.source.trim().is_empty() {
        output_flow(&c.id)
    } else {
        c.source.trim().to_string()
    }
}

/// Ce qui part, d'après la source (ou le format maison quand elle n'est pas là) : entrelacé
/// désentrelacé à la cadence des trames, sauf format demandé.
fn target_of(c: &OutputCfg, feed: &Video) -> Video {
    match video_of(&c.format) {
        Some(v) => v,
        None if feed.interlaced => Video { rate: (feed.rate.0 * 2, feed.rate.1), interlaced: false, ..*feed },
        None => *feed,
    }
}

/// Une image noire v210.
fn black_v210(v: &Video) -> Vec<u8> {
    let stride = strata_mxl::v210_stride(v.width);
    let uyvy: Vec<u8> = (0..v.width as usize * 2).map(|i| if i % 2 == 0 { 128 } else { 16 }).collect();
    let mut line = vec![0u8; stride];
    strata_mxl::uyvy_to_v210(&uyvy, &mut line, v.width as usize);
    line.repeat(v.height as usize)
}

fn run_output(c: &OutputCfg, env: &Env, stop: &AtomicBool, status: &Mutex<HashMap<String, OutputStatus>>) {
    let set = |f: &dyn Fn(&mut OutputStatus)| f(status.lock().unwrap().entry(c.id.clone()).or_default());
    let src = source_of(c);
    let (vid, aid) = (strata_mxl::flow_id(&src), strata_mxl::flow_id(&audio_of(&src)));
    let hls_dir = env.hls_dir.join(&c.id);
    if c.destinations.iter().any(|d| d.enabled && d.url == "hls") {
        let _ = std::fs::create_dir_all(&hls_dir);
    }
    let (mut encoder, mut node) = encoder_of(c, env);
    let mut failures = 0u32;
    while !stop.load(Ordering::Relaxed) {
        // La forme de ce qu'on donne à l'encodeur : celle de la source si elle est là.
        let reader = if env.mxl.is_active(vid) { env.mxl.video_reader(vid).ok() } else { None };
        let feed = match &reader {
            Some(r) => r.video,
            None => video_of(&c.format).unwrap_or(env.house),
        };
        let target = target_of(c, &feed);
        set(&|s| {
            s.encoder = Some(encoder.clone());
            s.format = Some(format_name(&target));
            if s.state != State::Error {
                set_state(s, State::Starting, |s| (&mut s.state, &mut s.since));
            }
        });
        let started = Instant::now();
        let r = feed_encoder(c, env, &feed, &target, &encoder, node.as_deref(), reader, vid, aid, &hls_dir, stop, &set);
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match r {
            // La source a changé de forme : on reprend aussitôt.
            Ok(()) => failures = 0,
            Err(e) => {
                // Le GPU n'encode pas (pilote, session refusée) : x264 en `auto`.
                if c.encoder == "auto" && encoder != "x264" && started.elapsed() < Duration::from_secs(5) {
                    tracing::warn!("network output {}: {encoder} failed ({e}), using x264", c.name);
                    (encoder, node) = ("x264".into(), None);
                    continue;
                }
                failures += 1;
                set(&|s| {
                    set_state(s, State::Error, |s| (&mut s.state, &mut s.since));
                    s.error = Some(e.clone());
                });
                pause(stop, backoff(failures));
            }
        }
    }
}

/// Change l'état d'une sortie.
type OutSetter<'a> = &'a dyn Fn(&dyn Fn(&mut OutputStatus));

/// Une destination en marche : son ffmpeg reçoit le TS par un canal.
struct DestRun {
    tx: mpsc::SyncSender<Arc<Vec<u8>>>,
    thread: Option<JoinHandle<()>>,
}

#[allow(clippy::too_many_arguments)]
fn feed_encoder(
    c: &OutputCfg,
    env: &Env,
    feed: &Video,
    target: &Video,
    encoder: &str,
    node: Option<&str>,
    mut reader: Option<strata_mxl::VideoReader>,
    vid: strata_mxl::Uuid,
    aid: strata_mxl::Uuid,
    hls_dir: &Path,
    stop: &AtomicBool,
    set: OutSetter,
) -> Result<(), String> {
    let mut p = ffmpeg::spawn(&encoder_args(c, feed, target, encoder, node), true, true, ffmpeg::Fd3::Write)?;
    let stdin = p.child.stdin.take().ok_or("no encoder input")?;
    let audio_out = p.fd3.take().ok_or("no encoder audio input")?;
    let stdout = p.child.stdout.take().ok_or("no encoder output")?;
    // Les destinations, chacune avec son ffmpeg.
    let dstop = Arc::new(AtomicBool::new(false));
    let dstatus: Arc<Mutex<Vec<DestStatus>>> =
        Arc::new(Mutex::new(c.destinations.iter().filter(|d| d.enabled).map(|d| DestStatus { id: d.id.clone(), ..Default::default() }).collect()));
    let mut dests: Vec<DestRun> = Vec::new();
    for (i, d) in c.destinations.iter().filter(|d| d.enabled).enumerate() {
        let (tx, rx) = mpsc::sync_channel::<Arc<Vec<u8>>>(512);
        let (d, dstop, dstatus, dir) = (d.clone(), dstop.clone(), dstatus.clone(), hls_dir.to_path_buf());
        let thread = std::thread::Builder::new().name(format!("dest-{}", d.id)).spawn(move || run_destination(&d, &dir, rx, &dstop, &dstatus, i)).ok();
        dests.push(DestRun { tx, thread });
    }
    // Le TS de l'encodeur, recopié à chaque destination ; une destination en retard perd ses
    // paquets, pas les autres.
    let senders: Vec<mpsc::SyncSender<Arc<Vec<u8>>>> = dests.iter().map(|d| d.tx.clone()).collect();
    let fanout = std::thread::spawn(move || {
        let mut out = stdout;
        let mut buf = vec![0u8; 188 * 64];
        loop {
            match out.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let chunk = Arc::new(buf[..n].to_vec());
                    for s in &senders {
                        let _ = s.try_send(chunk.clone());
                    }
                }
            }
        }
    });
    // L'image et le son, écrits par deux fils : ffmpeg lit ses deux entrées à son rythme.
    let (vtx, vrx) = mpsc::sync_channel::<Arc<Vec<u8>>>(3);
    let (atx, arx) = mpsc::sync_channel::<Vec<u8>>(16);
    let vwriter = std::thread::spawn(move || {
        let mut w = stdin;
        while let Ok(f) = vrx.recv() {
            if w.write_all(&f).is_err() {
                break;
            }
        }
    });
    let awriter = std::thread::spawn(move || {
        let mut w = audio_out;
        while let Ok(b) = arx.recv() {
            if w.write_all(&b).is_err() {
                break;
            }
        }
    });
    let period = Duration::from_secs_f64(feed.rate.1.max(1) as f64 / feed.rate.0.max(1) as f64);
    let black = Arc::new(black_v210(feed));
    let mut last: Arc<Vec<u8>> = black.clone();
    let mut last_live = Instant::now() - Duration::from_secs(10);
    let mut areader: Option<strata_mxl::AudioReader> = None;
    let (mut acc, mut frames, mut missing) = (0u64, 0u64, 0u64);
    let mut next = Instant::now();
    let mut tick = Instant::now();
    let mut result = Ok(());
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        // La source : prise quand elle apparaît ; une autre forme fait relancer l'encodeur.
        if env.mxl.is_active(vid) {
            if reader.is_none() {
                reader = env.mxl.video_reader(vid).ok();
            }
            if reader.as_ref().is_some_and(|r| r.video != *feed) {
                break;
            }
            // Le flux recréé à un autre format (une entrée qui a reconnu sa source) : on suit.
            if tick.elapsed() >= Duration::from_secs(1) && env.mxl.video_of(vid).is_some_and(|v| v != *feed) {
                break;
            }
        } else {
            reader = None;
        }
        if env.mxl.is_active(aid) {
            if areader.is_none() {
                areader = env.mxl.audio_reader(aid).ok();
            }
        } else {
            areader = None;
        }
        if let Some(v210) = reader.as_ref().and_then(|r| r.latest(period * 3)) {
            last = Arc::new(v210);
            last_live = Instant::now();
        } else if last_live.elapsed() > Duration::from_secs(1) {
            last = black.clone();
        }
        let live = last_live.elapsed() <= Duration::from_secs(1);
        // Le son de cette image (48 000 × durée, au fil des fractions).
        acc += 48_000 * feed.rate.1.max(1) as u64;
        let n = (acc / feed.rate.0.max(1) as u64) as usize;
        acc %= feed.rate.0.max(1) as u64;
        let pcm = areader.as_mut().and_then(|r| {
            let ch = r.channels.max(1);
            r.next_interleaved(n).map(|s| if ch == 2 { s } else { (0..n).flat_map(|k| [s[k * ch], s[k * ch + (ch > 1) as usize]]).collect() })
        });
        if pcm.is_none() && areader.is_some() {
            missing += 1;
        }
        let pcm = pcm.unwrap_or_else(|| vec![0f32; n * 2]);
        let bytes: Vec<u8> = pcm.iter().flat_map(|x| x.to_le_bytes()).collect();
        // Sans attendre : au démarrage, ffmpeg ouvre une entrée puis l'autre ; un encodeur en retard
        // perd des images plutôt que de bloquer la cadence.
        if gone(atx.try_send(bytes)) || gone(vtx.try_send(last.clone())) || !p.running() {
            result = Err(p.log.lock().unwrap().last_error().unwrap_or_else(|| "the encoder stopped".into()));
            break;
        }
        frames += 1;
        if tick.elapsed() >= Duration::from_secs(1) {
            tick = Instant::now();
            let log = p.log.lock().unwrap().clone();
            let ds = dstatus.lock().unwrap().clone();
            set(&|s| {
                s.frames = frames;
                s.fps = log.num("fps").unwrap_or(0.0);
                s.kbps = log.num("bitrate").unwrap_or(0.0);
                s.live = live;
                s.audio_missing = missing;
                s.destinations = ds.clone();
                let any = ds.iter().any(|d| d.state == State::Running);
                set_state(s, if any { State::Running } else { State::Starting }, |s| (&mut s.state, &mut s.since));
                s.error = None;
            });
        }
        // À la cadence de la source ; un retard de plus d'une seconde est oublié.
        next += period;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else if now - next > Duration::from_secs(1) {
            next = now;
        }
    }
    drop(vtx);
    drop(atx);
    p.stop();
    let _ = vwriter.join();
    let _ = awriter.join();
    let _ = fanout.join();
    dstop.store(true, Ordering::Relaxed);
    for d in dests {
        drop(d.tx);
        if let Some(t) = d.thread {
            let _ = t.join();
        }
    }
    result
}

/// Le fil qui écrit à ffmpeg est parti (ffmpeg arrêté).
fn gone<T>(r: Result<(), mpsc::TrySendError<T>>) -> bool {
    matches!(r, Err(mpsc::TrySendError::Disconnected(_)))
}

fn run_destination(d: &Destination, hls_dir: &Path, rx: mpsc::Receiver<Arc<Vec<u8>>>, stop: &AtomicBool, status: &Mutex<Vec<DestStatus>>, i: usize) {
    let set = |f: &dyn Fn(&mut DestStatus)| {
        if let Some(s) = status.lock().unwrap().get_mut(i) {
            f(s)
        }
    };
    let mut failures = 0u32;
    'outer: while !stop.load(Ordering::Relaxed) {
        set(&|s| set_state(s, State::Starting, |s| (&mut s.state, &mut s.since)));
        let mut p = match ffmpeg::spawn(&destination_args(d, hls_dir), true, false, ffmpeg::Fd3::None) {
            Ok(p) => p,
            Err(e) => {
                set(&|s| {
                    set_state(s, State::Error, |s| (&mut s.state, &mut s.since));
                    s.error = Some(e.clone());
                });
                break;
            }
        };
        let mut stdin = p.child.stdin.take();
        let mut tick = Instant::now();
        let started = Instant::now();
        loop {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(chunk) => {
                    if stdin.as_mut().is_none_or(|w| w.write_all(&chunk).is_err()) {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break 'outer,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if stop.load(Ordering::Relaxed) {
                break 'outer;
            }
            if !p.running() {
                break;
            }
            if tick.elapsed() >= Duration::from_secs(1) {
                tick = Instant::now();
                let log = p.log.lock().unwrap().clone();
                let kbps = log.num("bitrate").unwrap_or(0.0);
                // HLS n'a pas de taille (plusieurs fichiers) : le temps écrit suffit.
                let sending = log.num("total_size").unwrap_or(0.0) > 0.0 || log.num("out_time_us").unwrap_or(0.0) > 0.0;
                set(&|s| {
                    s.kbps = kbps;
                    if sending {
                        set_state(s, State::Running, |s| (&mut s.state, &mut s.since));
                        s.error = None;
                    }
                });
                if sending && started.elapsed() > Duration::from_secs(10) {
                    failures = 0;
                }
            }
        }
        drop(stdin);
        p.stop();
        let why = p.log.lock().unwrap().last_error().unwrap_or_else(|| "the destination closed the connection".into());
        failures += 1;
        set(&|s| {
            set_state(s, State::Error, |s| (&mut s.state, &mut s.since));
            s.error = Some(why.clone());
            s.restarts += 1;
        });
        // En attendant de réessayer, le TS qui arrive est jeté.
        let end = Instant::now() + backoff(failures);
        while Instant::now() < end && !stop.load(Ordering::Relaxed) {
            if let Err(mpsc::RecvTimeoutError::Disconnected) = rx.recv_timeout(Duration::from_millis(100)) {
                break 'outer;
            }
        }
    }
    let _ = &stop;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_commands() {
        let v = Video { width: 1920, height: 1080, rate: (25, 1), interlaced: true };
        let c = InputCfg { id: "a".into(), url: "rtmp://0.0.0.0:1935/live/a".into(), relay: true, relay_port: 20000, ..Default::default() };
        let a = input_args(&c, &v, true, None).join(" ");
        assert!(a.starts_with("-listen 1 "));
        assert!(a.contains("fps=25/1,scale=1920:1080:interl=-1,setfield=tff"));
        assert!(a.contains("-c:v v210 -f rawvideo pipe:1"));
        assert!(a.contains("-c:a pcm_f32le -flush_packets 1 -f f32le pipe:3"));
        assert!(a.ends_with("udp://127.0.0.1:20000?pkt_size=1316"));
        let c = InputCfg { url: "srt://10.0.0.1:9000".into(), ..Default::default() };
        let a = input_args(&c, &Video { interlaced: false, rate: (50, 1), ..v }, false, None).join(" ");
        assert!(a.starts_with("-fflags"));
        assert!(a.contains("yadif=mode=send_frame:deint=interlaced,fps=50/1"));
        assert!(!a.contains("pipe:3") && !a.contains("udp://"));
        let cuda = ("cuda".to_string(), None);
        assert!(input_args(&c, &v, false, Some(&cuda)).join(" ").starts_with("-hwaccel cuda -fflags"));
    }

    #[test]
    fn the_source_sets_scan_and_rate() {
        let p = source_video("Video: h264 (High), yuv420p(progressive), 1920x1080 [SAR 1:1 DAR 16:9], 25 fps, 25 tbr, 90k tbn").unwrap();
        assert_eq!(format_name(&p), "1080p25");
        let i = source_video("Video: h264 (High), yuv420p(tv, bt709, top first), 1920x1080, 25 fps, 50 tbr, 90k tbn").unwrap();
        assert_eq!(format_name(&i), "1080i50");
        let n = source_video("Video: h264 (Main), yuv420p(tv, bt709), 1280x720, 59.94 fps, 59.94 tbr").unwrap();
        assert_eq!(format_name(&n), "1080p59.94");
        assert!(source_video("Video: h264, yuv420p, 1920x1080").is_none());
    }

    #[test]
    fn output_commands() {
        let feed = Video { width: 1920, height: 1080, rate: (25, 1), interlaced: true };
        let c = OutputCfg { id: "p".into(), video_kbps: 6000, ..Default::default() };
        let target = target_of(&c, &feed);
        assert_eq!(format_name(&target), "1080p50");
        let a = encoder_args(&c, &feed, &target, "x264", None).join(" ");
        assert!(a.contains("-f v210 -video_size 1920x1080 -framerate 25/1"));
        assert!(a.contains("format=yuv422p,setfield=tff,bwdif=mode=send_field,fps=50/1,format=yuv420p"));
        assert!(a.contains("-b:v 6000k -maxrate 6000k -bufsize 12000k -g 100"));
        assert!(a.ends_with("-f mpegts -muxdelay 0 -flush_packets 1 pipe:1"));
        let c720 = OutputCfg { format: "720p50".into(), ..c.clone() };
        let a = encoder_args(&c720, &feed, &target_of(&c720, &feed), "vaapi", Some("/dev/dri/renderD129")).join(" ");
        assert!(a.starts_with("-vaapi_device /dev/dri/renderD129"));
        assert!(a.contains("scale=1280:720,format=nv12,hwupload -c:v h264_vaapi"));
        let yt = Destination { url: "rtmp://a.rtmp.youtube.com/live2/".into(), key: "abcd-efgh".into(), ..Default::default() };
        assert_eq!(destination_url(&yt), "rtmp://a.rtmp.youtube.com/live2/abcd-efgh");
        let srt = Destination { url: "srt://cdn.example:9000".into(), key: "live/prog".into(), ..Default::default() };
        assert_eq!(destination_url(&srt), "srt://cdn.example:9000?mode=caller&pkt_size=1316&streamid=live/prog");
        let a = destination_args(&yt, Path::new("/x")).join(" ");
        assert!(a.contains("-c copy -f flv -flvflags no_duration_filesize rtmp://"));
        let hls = Destination { url: "hls".into(), ..Default::default() };
        assert!(destination_args(&hls, Path::new("/d/hls/p")).join(" ").ends_with("-hls_segment_filename /d/hls/p/seg%05d.ts /d/hls/p/index.m3u8"));
    }
}
