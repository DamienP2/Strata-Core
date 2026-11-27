//! Les flux réseau du serveur, tenus par le Core (Réglages › Flux réseau) :
//!
//! - **Entrées** (SRT, RTMP, RTSP, HLS, UDP/RTP en TS, RIST) : décodées par ffmpeg, écrites en MXL
//!   (`stream-in/<id>` : l'image en v210 ; `stream-in/<id>/audio` : le son, float32 48 kHz). Les
//!   applis les lisent comme une entrée SDI. Le flux d'origine est aussi remis tel quel en local
//!   (TS sur `udp://127.0.0.1:<port>`) pour une appli qui enregistre ou relaie sans décoder.
//! - **Sorties** : le Core lit un flux MXL (celui qu'une appli écrit, `stream-out/<id>`, ou une
//!   entrée du serveur), l'encode une fois (GPU si possible) et l'envoie à chaque destination
//!   (RTMP(S), SRT, UDP, RIST, HLS servi par le Core). Du noir et du silence quand rien n'arrive.
//!   Une destination qui tombe est relancée seule, sans couper les autres.
//!
//! Les réglages sont dans `streams.json` (droits 0600 : les clés de diffusion).
pub mod api;
pub mod engine;
pub mod ffmpeg;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Une entrée réseau.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct InputCfg {
    /// `[a-z0-9-]`, stable : il fait l'identifiant MXL.
    pub id: String,
    pub name: String,
    /// `srt://…`, `rtmp://…`, `rtsp://…`, `https://…/index.m3u8`, `udp://@239.1.1.1:5000`.
    /// RTMP sur `0.0.0.0` : le Core attend qu'on lui pousse le flux.
    pub url: String,
    pub enabled: bool,
    /// La vidéo écrite en MXL (« 1080i50 ») ; vide : comme la source (sa cadence et son balayage,
    /// en 1080) ; `house` : le format maison.
    pub format: String,
    /// Le flux d'origine remis en local (`udp://127.0.0.1:<relay_port>`, en TS).
    pub relay: bool,
    /// Son port (donné par le Core quand il manque).
    pub relay_port: u16,
}

/// Une destination d'une sortie.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Destination {
    pub id: String,
    pub name: String,
    /// `rtmp://a.rtmp.youtube.com/live2`, `srt://cdn:9000`, `udp://239.1.1.1:5000`, ou `hls`.
    pub url: String,
    /// La clé de diffusion (ajoutée à l'URL RTMP, `streamid` en SRT).
    pub key: String,
    pub enabled: bool,
}

/// Une sortie réseau.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct OutputCfg {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// Ce qui part : vide, le flux `stream-out/<id>` qu'une appli écrit ; sinon un flux du
    /// serveur (`stream-in/<id>`, `sdi/<carte>/<n>`, `2110/<flux>`).
    pub source: String,
    /// La vidéo envoyée (« 1080p50 », « 720p50 ») ; vide : celle de la source (entrelacé
    /// désentrelacé).
    pub format: String,
    pub video_kbps: u32,
    pub audio_kbps: u32,
    /// `auto` (GPU si possible), `nvenc`, `vaapi`, `x264`.
    pub encoder: String,
    pub destinations: Vec<Destination>,
    /// L'appli qui l'a déclarée (API des applis) ; None : réglée à la main.
    pub app: Option<String>,
}

impl Default for OutputCfg {
    fn default() -> Self {
        OutputCfg {
            id: String::new(),
            name: String::new(),
            enabled: true,
            source: String::new(),
            format: String::new(),
            video_kbps: 6000,
            audio_kbps: 192,
            encoder: "auto".into(),
            destinations: Vec::new(),
            app: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct StreamsConfig {
    pub inputs: Vec<InputCfg>,
    pub outputs: Vec<OutputCfg>,
}

/// Le premier port du relais local des entrées.
pub const RELAY_BASE: u16 = 20000;

impl StreamsConfig {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("streams.json")
    }

    pub fn load(data_dir: &Path) -> StreamsConfig {
        std::fs::read_to_string(Self::path(data_dir)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) -> Result<(), String> {
        let p = Self::path(data_dir);
        let tmp = p.with_extension("json.new");
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)
                .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
            f.write_all(json.as_bytes()).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, &p).map_err(|e| format!("cannot write {}: {e}", p.display()))
    }

    /// Vérifie et complète : identifiants, noms, ports du relais (uniques).
    pub fn check(&mut self) -> Result<(), String> {
        let valid_id = |id: &str| !id.is_empty() && id.len() <= 48 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        let mut ids = std::collections::HashSet::new();
        let mut ports = std::collections::HashSet::new();
        for i in &mut self.inputs {
            i.id = i.id.trim().to_ascii_lowercase();
            i.name = i.name.trim().to_string();
            i.url = i.url.trim().to_string();
            if !valid_id(&i.id) || !ids.insert(format!("in/{}", i.id)) {
                return Err(format!("input {:?}: give it a unique id (a-z, 0-9, -)", i.id));
            }
            if i.name.is_empty() {
                i.name = i.id.clone();
            }
            if scheme(&i.url).is_none() {
                return Err(format!("{}: not a stream address (srt://, rtmp://, rtsp://, http(s)://, udp://, rtp://, rist://)", i.name));
            }
            if !matches!(i.format.trim(), "" | "house") && crate::hardware::parse_format(&i.format).is_none() {
                return Err(format!("{}: unknown format {}", i.name, i.format));
            }
            if i.relay_port != 0 && !ports.insert(i.relay_port) {
                return Err(format!("{}: relay port {} is already used", i.name, i.relay_port));
            }
        }
        for o in &mut self.outputs {
            o.id = o.id.trim().to_ascii_lowercase();
            o.name = o.name.trim().to_string();
            if !valid_id(&o.id) || !ids.insert(format!("out/{}", o.id)) {
                return Err(format!("output {:?}: give it a unique id (a-z, 0-9, -)", o.id));
            }
            if o.name.is_empty() {
                o.name = o.id.clone();
            }
            if !o.format.trim().is_empty() && crate::hardware::parse_format(&o.format).is_none_or(|f| f.3) {
                return Err(format!("{}: {} is not a progressive format (1080p50, 720p50…)", o.name, o.format));
            }
            if !["auto", "nvenc", "vaapi", "x264"].contains(&o.encoder.as_str()) {
                o.encoder = "auto".into();
            }
            o.video_kbps = o.video_kbps.clamp(300, 100_000);
            o.audio_kbps = o.audio_kbps.clamp(32, 512);
            let mut dids = std::collections::HashSet::new();
            for (n, d) in o.destinations.iter_mut().enumerate() {
                d.url = d.url.trim().to_string();
                d.key = d.key.trim().to_string();
                if d.id.trim().is_empty() {
                    d.id = format!("d{}", n + 1);
                }
                if !dids.insert(d.id.clone()) {
                    return Err(format!("{}: two destinations are named {}", o.name, d.id));
                }
                if d.url != "hls" && !matches!(scheme(&d.url), Some("rtmp" | "rtmps" | "srt" | "udp" | "rtp" | "rist")) {
                    return Err(format!("{}: {} is not a destination (rtmp://, rtmps://, srt://, udp://, rtp://, rist://, or hls)", o.name, d.url));
                }
            }
        }
        // Les relais sans port : les suivants libres.
        let mut next = RELAY_BASE;
        for i in &mut self.inputs {
            if i.relay && i.relay_port == 0 {
                while ports.contains(&next) {
                    next += 2;
                }
                i.relay_port = next;
                ports.insert(next);
            }
        }
        Ok(())
    }
}

/// L'encodeur du GPU, cherché une fois : `nvenc`, ou `vaapi` et son nœud de rendu.
static GPU: std::sync::OnceLock<Option<(String, Option<String>)>> = std::sync::OnceLock::new();

fn gpu_encoder() -> Option<(String, Option<String>)> {
    GPU.get_or_init(|| {
        let gpus = crate::hardware::gpu::detect();
        let h264 = |g: &&crate::hardware::gpu::Gpu| g.ready && g.encode.iter().any(|c| c == "h264");
        if let Some(_g) = gpus.iter().filter(h264).find(|g| g.api == Some("nvenc")) {
            return Some(("nvenc".to_string(), None));
        }
        gpus.iter().filter(h264).find(|g| g.api == Some("vaapi")).map(|g| ("vaapi".to_string(), g.render_node.clone()))
    })
    .clone()
}

/// Le format maison en 1080 : entrelacé à 25 et 29,97 images/s, progressif sinon.
pub fn house_video(hw: &crate::hardware::HardwareConfig) -> strata_mxl::Video {
    let (n, d) = hw.clock.house.rate.ratio();
    let interlaced = matches!((n, d), (25, 1) | (30000, 1001));
    strata_mxl::Video { width: 1920, height: 1080, rate: (n as u32, d as u32), interlaced }
}

/// Tient les flux réseau en accord avec leurs réglages (bloquant).
pub fn reconcile(s: &crate::state::AppState) {
    let cfg = StreamsConfig::load(&s.data_dir);
    let mxl = match s.mxl.get() {
        Some(Ok(m)) => Some(m.clone()),
        _ => None,
    };
    // Le GPU (cherché une fois) : il décode les entrées et encode les sorties.
    let needs_gpu = cfg.inputs.iter().any(|i| i.enabled) || cfg.outputs.iter().any(|o| o.enabled && matches!(o.encoder.as_str(), "auto" | "vaapi"));
    let env = mxl.map(|mxl| engine::Env {
        mxl,
        house: house_video(&crate::hardware::HardwareConfig::load(&s.data_dir)),
        hls_dir: s.data_dir.join("streams").join("hls"),
        gpu: if needs_gpu { gpu_encoder() } else { None },
    });
    s.streams.reconcile(&cfg, env);
}

/// Le protocole d'une adresse (`srt`, `rtmp`, `https`…), s'il est connu.
pub fn scheme(url: &str) -> Option<&'static str> {
    let s = url.split_once("://")?.0.to_ascii_lowercase();
    ["srt", "rtmp", "rtmps", "rtsp", "rtsps", "http", "https", "udp", "rtp", "rist"].into_iter().find(|k| *k == s)
}

/// Les noms des flux MXL.
pub fn input_flow(id: &str) -> String {
    format!("stream-in/{id}")
}

pub fn output_flow(id: &str) -> String {
    format!("stream-out/{id}")
}

/// Le flux audio qui va avec un flux vidéo.
pub fn audio_of(flow: &str) -> String {
    format!("{flow}/audio")
}

/// Une adresse de flux pour le journal : sans identifiants ni paramètres (mot de passe RTSP,
/// `passphrase` SRT, clé de diffusion).
pub fn redacted(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let rest = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
    let (host, has_query) = match rest.split_once('?') {
        Some((h, _)) => (h, true),
        None => (rest, false),
    };
    // Le chemin d'une adresse RTMP porte la clé : seul le premier segment reste.
    let host = if scheme.starts_with("rtmp") { host.split('/').take(2).collect::<Vec<_>>().join("/") } else { host.to_string() };
    let sep = if scheme.is_empty() { "" } else { "://" };
    format!("{scheme}{sep}{host}{}", if has_query { "?…" } else { "" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_checked_and_completed() {
        let mut c = StreamsConfig {
            inputs: vec![
                InputCfg { id: "Studio-A".into(), url: "srt://10.0.0.1:9000".into(), enabled: true, relay: true, ..Default::default() },
                InputCfg { id: "cam2".into(), url: "rtmp://0.0.0.0:1935/live/cam2".into(), relay: true, ..Default::default() },
            ],
            outputs: vec![OutputCfg {
                id: "prog".into(),
                video_kbps: 50,
                encoder: "quantum".into(),
                destinations: vec![
                    Destination { url: "rtmp://a.rtmp.youtube.com/live2".into(), key: " abcd ".into(), enabled: true, ..Default::default() },
                    Destination { url: "hls".into(), enabled: true, ..Default::default() },
                ],
                ..Default::default()
            }],
        };
        c.check().unwrap();
        assert_eq!(c.inputs[0].id, "studio-a");
        assert_eq!(c.inputs[0].name, "studio-a");
        assert_eq!((c.inputs[0].relay_port, c.inputs[1].relay_port), (20000, 20002));
        let o = &c.outputs[0];
        assert_eq!((o.video_kbps, o.encoder.as_str()), (300, "auto"));
        assert_eq!((o.destinations[0].id.as_str(), o.destinations[0].key.as_str(), o.destinations[1].id.as_str()), ("d1", "abcd", "d2"));
        let mut bad = c.clone();
        bad.inputs[1].url = "ftp://x".into();
        assert!(bad.check().is_err());
        let mut bad = c.clone();
        bad.outputs[0].format = "1080i50".into();
        assert!(bad.check().is_err());
        let mut bad = c.clone();
        bad.outputs[0].destinations[0].url = "http://x".into();
        assert!(bad.check().is_err());
    }
}
