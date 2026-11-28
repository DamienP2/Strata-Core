//! Les flux réseau du serveur, tenus par le Core (Réglages › Flux réseau) :
//!
//! - **Entrées** (SRT, RTMP, RTSP, HLS, UDP/RTP en TS, RIST) : décodées par ffmpeg, écrites en MXL
//!   (`stream-in/<id>` : l'image en v210 ; `stream-in/<id>/audio` : le son, float32 48 kHz). Les
//!   applis les lisent comme une entrée SDI. Le flux d'origine est aussi remis tel quel en local
//!   (TS sur `udp://127.0.0.1:<port>`) pour une appli qui enregistre ou relaie sans décoder.
//! - **Destinations** (les sorties) : chacune indépendante. Le Core lit un flux MXL (celui qu'une
//!   appli écrit, `stream-out/<id>`, ou une entrée du serveur), l'encode (GPU si possible) et
//!   l'envoie à une adresse (RTMP(S), SRT, UDP, RIST, HLS ou lecteur HTML5 servis par le Core).
//!   Du noir et du silence quand rien n'arrive. Une appli qui veut le même flux à plusieurs
//!   endroits déclare plusieurs destinations.
//!
//! Les réglages sont dans `streams.json` (droits 0600 : les clés de diffusion).
pub mod api;
pub mod engine;
pub mod ffmpeg;
pub mod live;

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
    /// Les réglages propres au protocole.
    pub opts: ProtoOpts,
}

/// Les réglages propres à chaque protocole (passés à ffmpeg en options, jamais dans l'adresse).
/// Vide ou 0 : la valeur par défaut.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct ProtoOpts {
    /// SRT : caller, listener, rendezvous.
    pub srt_mode: String,
    /// SRT : latence (ms ; 120 par défaut).
    pub srt_latency_ms: u32,
    /// SRT : phrase de chiffrement (10 à 79 caractères).
    pub srt_passphrase: String,
    /// SRT : longueur de la clé (16, 24, 32 octets).
    pub srt_key_len: u8,
    /// SRT : identifiant de flux (en sortie : la clé de la destination s'il est vide).
    pub srt_stream_id: String,
    /// RTSP : tcp, udp, http.
    pub rtsp_transport: String,
    /// RTMP : écoute (on pousse le flux vers le Core) ; sinon d'après l'adresse (0.0.0.0).
    pub rtmp_listen: bool,
    /// UDP, RTP, RIST : l'adresse locale (interface) pour le multicast.
    pub local_address: String,
    /// UDP, RTP : les sources acceptées (multicast SSM), séparées par des virgules.
    pub sources: String,
    /// UDP, RTP (sortie) : TTL du multicast.
    pub ttl: u8,
    /// UDP, RTP : tampon de réception (Ko).
    pub buffer_kb: u32,
    /// RIST : simple, main, advanced.
    pub rist_profile: String,
    /// RIST : tampon de reprise (ms).
    pub rist_buffer_ms: u32,
    /// RIST : secret (profil main), et chiffrement (128 ou 256).
    pub rist_secret: String,
    pub rist_encryption: u16,
    /// HLS (sortie) : durée d'un segment (s) et segments dans la liste.
    pub hls_segment_s: u8,
    pub hls_list_size: u8,
    /// Délai sans données avant d'abandonner et de réessayer (s ; 10 par défaut).
    pub timeout_s: u32,
}

impl ProtoOpts {
    /// Vérifie les valeurs (le nom du flux pour les messages).
    pub fn check(&mut self, name: &str) -> Result<(), String> {
        let clean = |s: &mut String| *s = s.trim().to_string();
        for s in [
            &mut self.srt_mode,
            &mut self.srt_passphrase,
            &mut self.srt_stream_id,
            &mut self.rtsp_transport,
            &mut self.local_address,
            &mut self.sources,
            &mut self.rist_profile,
            &mut self.rist_secret,
        ] {
            clean(s);
        }
        let bad = |what: &str| Err(format!("{name}: {what}"));
        if !["", "caller", "listener", "rendezvous"].contains(&self.srt_mode.as_str()) {
            return bad("the SRT mode is caller, listener or rendezvous");
        }
        if !self.srt_passphrase.is_empty() && !(10..=79).contains(&self.srt_passphrase.chars().count()) {
            return bad("the SRT passphrase has 10 to 79 characters");
        }
        if ![0, 16, 24, 32].contains(&self.srt_key_len) {
            return bad("the SRT key length is 16, 24 or 32");
        }
        if !["", "tcp", "udp", "http"].contains(&self.rtsp_transport.as_str()) {
            return bad("the RTSP transport is tcp, udp or http");
        }
        if !self.local_address.is_empty() && self.local_address.parse::<std::net::Ipv4Addr>().is_err() {
            return bad("the local address is an IPv4 address of this server");
        }
        if self.sources.split(',').map(str::trim).filter(|x| !x.is_empty()).any(|x| x.parse::<std::net::Ipv4Addr>().is_err()) {
            return bad("the sources are IPv4 addresses separated by commas");
        }
        if !["", "simple", "main", "advanced"].contains(&self.rist_profile.as_str()) {
            return bad("the RIST profile is simple, main or advanced");
        }
        if ![0, 128, 256].contains(&self.rist_encryption) {
            return bad("the RIST encryption is 128 or 256");
        }
        if self.text().any(|s| s.chars().any(char::is_control)) {
            return bad("no control characters");
        }
        self.srt_latency_ms = self.srt_latency_ms.min(10_000);
        self.rist_buffer_ms = self.rist_buffer_ms.min(30_000);
        self.buffer_kb = self.buffer_kb.min(512 * 1024);
        self.timeout_s = self.timeout_s.min(600);
        self.hls_segment_s = self.hls_segment_s.min(30);
        self.hls_list_size = self.hls_list_size.min(60);
        Ok(())
    }

    fn text(&self) -> impl Iterator<Item = &String> {
        [
            &self.srt_mode,
            &self.srt_passphrase,
            &self.srt_stream_id,
            &self.rtsp_transport,
            &self.local_address,
            &self.sources,
            &self.rist_profile,
            &self.rist_secret,
        ]
        .into_iter()
    }

    /// Les secrets, à masquer quand on les montre.
    pub fn mask(&mut self, mask: &str) {
        for s in [&mut self.srt_passphrase, &mut self.rist_secret] {
            if !s.is_empty() {
                *s = mask.into();
            }
        }
    }

    /// Les secrets masqués reprennent leur valeur gardée.
    pub fn keep(&mut self, old: Option<&ProtoOpts>, mask: &str) {
        if self.srt_passphrase == mask {
            self.srt_passphrase = old.map(|o| o.srt_passphrase.clone()).unwrap_or_default();
        }
        if self.rist_secret == mask {
            self.rist_secret = old.map(|o| o.rist_secret.clone()).unwrap_or_default();
        }
    }
}

/// Où part une sortie (pour le moteur ; l'ancien format des réglages en avait plusieurs).
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
    /// Les réglages propres au protocole.
    pub opts: ProtoOpts,
}

/// Une sortie réseau (une destination, dans l'interface) : ce qu'elle envoie, comment c'est
/// encodé, où ça part.
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
    /// `rtmp://a.rtmp.youtube.com/live2`, `srt://cdn:9000`, `udp://239.1.1.1:5000`, `hls`, `html5`.
    pub url: String,
    /// La clé de diffusion (ajoutée à l'URL RTMP, `streamid` en SRT).
    pub key: String,
    /// Les réglages propres au protocole.
    pub opts: ProtoOpts,
    /// L'ancien format (plusieurs destinations par sortie) : lu, puis découpé en sorties.
    #[serde(skip_serializing)]
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
            url: String::new(),
            key: String::new(),
            opts: ProtoOpts::default(),
            destinations: Vec::new(),
            app: None,
        }
    }
}

impl OutputCfg {
    /// Où elle part, pour le moteur.
    pub fn target(&self) -> Destination {
        Destination { id: "d1".into(), name: self.name.clone(), url: self.url.clone(), key: self.key.clone(), enabled: true, opts: self.opts.clone() }
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
        let mut c: StreamsConfig = std::fs::read_to_string(Self::path(data_dir)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        c.split_old_outputs();
        c
    }

    /// L'ancien format : une sortie à plusieurs destinations devient une sortie par destination
    /// (la première garde l'identifiant, les autres `<sortie>-<destination>` et envoient le même
    /// flux : celui que l'appli écrit pour la première).
    pub fn split_old_outputs(&mut self) {
        let mut out = Vec::with_capacity(self.outputs.len());
        for mut o in std::mem::take(&mut self.outputs) {
            let old = std::mem::take(&mut o.destinations);
            if !o.url.is_empty() || old.is_empty() {
                out.push(o);
                continue;
            }
            let many = old.len() > 1;
            for (n, d) in old.into_iter().enumerate() {
                let mut x = o.clone();
                if n > 0 {
                    let tail: String = if d.id.is_empty() { format!("{}", n + 1) } else { d.id.to_ascii_lowercase() };
                    x.id = format!("{}-{}", o.id, tail);
                    if x.source.trim().is_empty() {
                        x.source = output_flow(&o.id);
                    }
                }
                if many {
                    let what = if d.name.is_empty() { d.url.split("://").next().unwrap_or("").to_uppercase() } else { d.name.clone() };
                    x.name = format!("{} · {}", o.name, what);
                }
                x.enabled = o.enabled && d.enabled;
                (x.url, x.key, x.opts) = (d.url, d.key, d.opts);
                out.push(x);
            }
        }
        self.outputs = out;
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
        self.split_old_outputs();
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
            let label = i.name.clone();
            i.opts.check(&label)?;
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
            o.url = o.url.trim().to_string();
            o.key = o.key.trim().to_string();
            if !matches!(o.url.as_str(), "hls" | "html5") && !matches!(scheme(&o.url), Some("rtmp" | "rtmps" | "srt" | "udp" | "rtp" | "rist")) {
                return Err(format!("{}: {} is not a destination (rtmp://, rtmps://, srt://, udp://, rtp://, rist://, hls or html5)", o.name, o.url));
            }
            let label = o.name.clone();
            o.opts.check(&label)?;
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
        Some(Ok(m)) => Ok(m.clone()),
        Some(Err(e)) => Err(e.clone()),
        None => Err("not opened yet".to_string()),
    };
    // Le GPU (cherché une fois) : il décode les entrées et encode les sorties.
    let needs_gpu = cfg.inputs.iter().any(|i| i.enabled) || cfg.outputs.iter().any(|o| o.enabled && matches!(o.encoder.as_str(), "auto" | "vaapi"));
    let env = mxl.map(|mxl| engine::Env {
        mxl,
        house: house_video(&crate::hardware::HardwareConfig::load(&s.data_dir)),
        hls_dir: s.data_dir.join("streams").join("hls"),
        live: s.streams.live.clone(),
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

/// Une seule écriture de `streams.json` à la fois (la page, les applis) : relu, changé, écrit
/// sous ce verrou.
pub fn write_lock() -> std::sync::MutexGuard<'static, ()> {
    static W: std::sync::Mutex<()> = std::sync::Mutex::new(());
    W.lock().unwrap_or_else(|e| e.into_inner())
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
                name: "Prog".into(),
                video_kbps: 50,
                encoder: "quantum".into(),
                // L'ancien format, découpé : une sortie par destination.
                destinations: vec![
                    Destination { url: "rtmp://a.rtmp.youtube.com/live2".into(), key: " abcd ".into(), enabled: true, ..Default::default() },
                    Destination { id: "web".into(), name: "Web".into(), url: "hls".into(), enabled: false, ..Default::default() },
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
        assert_eq!(c.outputs.len(), 2);
        assert_eq!((o.id.as_str(), o.name.as_str(), o.key.as_str(), o.enabled), ("prog", "Prog · RTMP", "abcd", true));
        let w = &c.outputs[1];
        assert_eq!((w.id.as_str(), w.name.as_str(), w.url.as_str(), w.enabled, w.video_kbps), ("prog-web", "Prog · Web", "hls", false, 300));
        assert_eq!(w.source, "stream-out/prog");
        assert!(!serde_json::to_string(&c).unwrap().contains("destinations"));
        let mut bad = c.clone();
        bad.inputs[1].url = "ftp://x".into();
        assert!(bad.check().is_err());
        let mut bad = c.clone();
        bad.outputs[0].format = "1080i50".into();
        assert!(bad.check().is_err());
        let mut bad = c.clone();
        bad.outputs[0].url = "http://x".into();
        assert!(bad.check().is_err());
    }
}
