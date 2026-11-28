//! Les flux vidéo du serveur en MXL (Media eXchange Layer, EBU DMF / Linux Foundation).
//!
//! Le Core écrit chaque entrée (SDI, ST 2110 reçu) en flux MXL ; les applis les lisent, et
//! écrivent leurs sorties en flux MXL que le Core envoie (SDI, ST 2110). Tout reste dans la
//! mémoire du serveur (un dossier tmpfs, le « domaine ») : rien ne passe par le réseau.
//!
//! - La bibliothèque `libmxl.so` est chargée au lancement : sans elle, le Core démarre quand même
//!   (et garde son partage `strata-share`).
//! - Vidéo en v210 (YCbCr 4:2:2 10 bits, 6 pixels sur 16 octets) ; un grain par image, ou par
//!   trame en entrelacé (MXL compte les trames : 1080i50, ce sont 50 grains de 540 lignes).
//! - Les grains sont rangés par leur instant (index TAI) : un lecteur demande « l'image de
//!   maintenant », pas « la suivante ».
//! - Chaque flux a un identifiant stable (UUID v5) : `sdi/<carte>/<n>` pour une entrée SDI, que
//!   les applis retrouvent d'un démarrage à l'autre.
pub mod anc;

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

pub use uuid::Uuid;

/// Les identifiants de Strata (UUID v5) : `Uuid::new_v5(&NAMESPACE, b"sdi/7b10a04e/1")`.
pub const NAMESPACE: Uuid = Uuid::from_u128(0x5a7a_7a00_c07e_4d6e_9e1a_5f3c_0de0_0001);

/// L'identifiant d'un flux de Strata, d'après son nom (`sdi/<carte>/<n>`, `sdi-out/<carte>/<n>`,
/// `2110/<flux>`).
pub fn flow_id(name: &str) -> Uuid {
    Uuid::new_v5(&NAMESPACE, name.to_ascii_lowercase().as_bytes())
}

/// Le domaine MXL du serveur : `STRATA_MXL_DOMAIN`, sinon `/dev/shm/mxl`.
pub fn domain() -> PathBuf {
    match std::env::var_os("STRATA_MXL_DOMAIN") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => PathBuf::from("/dev/shm/mxl"),
    }
}

/// Où chercher la bibliothèque : `STRATA_MXL_LIB`, celle posée avec le Core, puis le système.
fn libraries() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::env::var_os("STRATA_MXL_LIB").filter(|s| !s.is_empty()).map(PathBuf::from).into_iter().collect();
    // À côté du programme (`/opt/strata/bin/../lib`), pour une archive du Core qui l'embarque.
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.join("../lib/libmxl.so.1"))) {
        v.push(dir);
    }
    v.extend(["/opt/strata/lib/libmxl.so.1", "/usr/local/lib/libmxl.so.1", "libmxl.so.1", "libmxl.so"].map(PathBuf::from));
    v
}

/// Une instance MXL sur le domaine du serveur.
pub struct Mxl {
    inst: mxl::MxlInstance,
    domain: PathBuf,
    /// « 1.2.0 ».
    pub version: String,
    /// La bibliothèque chargée.
    pub library: PathBuf,
}

impl Mxl {
    /// Charge la bibliothèque et ouvre le domaine (créé s'il manque).
    pub fn open() -> Result<Mxl, String> {
        Self::open_at(&domain())
    }

    pub fn open_at(domain: &Path) -> Result<Mxl, String> {
        let mut last = "libmxl.so not found".to_string();
        for lib in libraries() {
            match mxl::load_api(&lib) {
                Ok(api) => {
                    let version = mxl::get_version(&api).map(|v| format!("{}.{}.{}", v.major, v.minor, v.bugfix)).unwrap_or_default();
                    std::fs::create_dir_all(domain).map_err(|e| format!("cannot create the MXL domain {}: {e}", domain.display()))?;
                    let d = domain.to_str().ok_or("the MXL domain is not UTF-8")?;
                    let inst = mxl::MxlInstance::new(api, d, "").map_err(|e| format!("cannot open the MXL domain {d}: {e}"))?;
                    return Ok(Mxl { inst, domain: domain.to_path_buf(), version, library: lib });
                }
                Err(e) => last = format!("{}: {e}", lib.display()),
            }
        }
        Err(last)
    }

    pub fn domain(&self) -> &Path {
        &self.domain
    }

    /// Crée (ou reprend) le flux vidéo `id` et rend son écrivain.
    pub fn video_writer(&self, id: Uuid, label: &str, v: &Video) -> Result<VideoWriter, String> {
        // Un ancien flux de ce nom, sans écrivain, à un autre format : effacé d'abord (sinon MXL le
        // rendrait tel quel).
        if !self.is_active(id) && self.video_of(id).is_some_and(|old| old != *v) {
            self.collect();
        }
        let def = video_flow_def(id, label, v);
        let (w, info, _) = self.inst.create_flow_writer(&def, None).map_err(|e| format!("cannot create the MXL flow {id}: {e}"))?;
        // MXL rend le flux existant s'il a encore un écrivain : il doit être à ce format.
        if let Some(actual) = self.video_of(id).filter(|a| a != v) {
            return Err(format!(
                "the MXL flow {id} is still open as {}x{}{} by another writer",
                actual.width,
                actual.height,
                if actual.interlaced { "i" } else { "p" }
            ));
        }
        let rate = info.common().grain_rate().map_err(|e| e.to_string())?;
        let grains = w.to_grain_writer().map_err(|e| e.to_string())?;
        Ok(VideoWriter { inst: self.inst.clone(), grains, rate, video: *v, last: None, scratch: Vec::new() })
    }

    /// Ouvre le flux vidéo `id` en lecture.
    pub fn video_reader(&self, id: Uuid) -> Result<VideoReader, String> {
        let def = self.inst.get_flow_def(&id.to_string()).map_err(|e| format!("no MXL flow {id}: {e}"))?;
        let video = Video::from_flow_def(&def).ok_or_else(|| format!("the MXL flow {id} is not v210 video"))?;
        let r = self.inst.create_flow_reader(&id.to_string()).map_err(|e| format!("cannot read the MXL flow {id}: {e}"))?;
        let grains = r.to_grain_reader().map_err(|e| e.to_string())?;
        let rate = grains.get_config_info().map_err(|e| e.to_string())?.common().grain_rate().map_err(|e| e.to_string())?;
        Ok(VideoReader { inst: self.inst.clone(), grains, rate, video, read_at: std::sync::atomic::AtomicU64::new(u64::MAX) })
    }

    /// Crée (ou reprend) le flux audio `id` (float32, 48 kHz, `channels` canaux).
    pub fn audio_writer(&self, id: Uuid, label: &str, channels: u32) -> Result<AudioWriter, String> {
        let def = audio_flow_def(id, label, channels);
        let (w, _, _) = self.inst.create_flow_writer(&def, None).map_err(|e| format!("cannot create the MXL flow {id}: {e}"))?;
        let samples = w.to_samples_writer().map_err(|e| e.to_string())?;
        let max = samples.get_max_write_length_samples().unwrap_or(4800).max(1);
        Ok(AudioWriter { inst: self.inst.clone(), samples, channels: channels as usize, next: None, max })
    }

    /// Ouvre le flux audio `id` en lecture.
    pub fn audio_reader(&self, id: Uuid) -> Result<AudioReader, String> {
        let def = self.inst.get_flow_def(&id.to_string()).map_err(|e| format!("no MXL flow {id}: {e}"))?;
        let v: serde_json::Value = serde_json::from_str(&def).map_err(|e| e.to_string())?;
        if v["media_type"] != "audio/float32" || v["sample_rate"]["numerator"].as_u64() != Some(AUDIO_RATE as u64) {
            return Err(format!("the MXL flow {id} is not 48 kHz float32 audio"));
        }
        let channels = v["channel_count"].as_u64().unwrap_or(2) as usize;
        let r = self.inst.create_flow_reader(&id.to_string()).map_err(|e| format!("cannot read the MXL flow {id}: {e}"))?;
        let samples = r.to_samples_reader().map_err(|e| e.to_string())?;
        Ok(AudioReader { inst: self.inst.clone(), samples, channels, next: None })
    }

    /// La vidéo d'un flux d'après sa description (elle change quand l'écrivain le recrée à un
    /// autre format) ; None : pas de flux, ou pas de la vidéo v210.
    pub fn video_of(&self, id: Uuid) -> Option<Video> {
        Video::from_flow_def(&self.inst.get_flow_def(&id.to_string()).ok()?)
    }

    /// Le nom qu'un flux se donne (`label` de sa description).
    pub fn flow_label(&self, id: Uuid) -> Option<String> {
        let def = self.inst.get_flow_def(&id.to_string()).ok()?;
        serde_json::from_str::<serde_json::Value>(&def).ok()?["label"].as_str().map(str::to_string)
    }

    /// Le flux existe et son écrivain est vivant.
    pub fn is_active(&self, id: Uuid) -> bool {
        self.inst.is_flow_active(&id.to_string()).unwrap_or(false)
    }

    /// Efface les flux dont l'écrivain a disparu.
    pub fn collect(&self) {
        let _ = self.inst.garbage_collect_flows();
    }
}

/// Une vidéo : taille de l'image entière, cadence en images par seconde, entrelacée ou non.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Video {
    pub width: u32,
    pub height: u32,
    pub rate: (u32, u32),
    pub interlaced: bool,
}

impl Video {
    /// Les lignes d'un grain : l'image, ou une trame en entrelacé.
    pub fn grain_lines(&self) -> u32 {
        if self.interlaced {
            self.height / 2
        } else {
            self.height
        }
    }

    fn from_flow_def(def: &str) -> Option<Video> {
        let v: serde_json::Value = serde_json::from_str(def).ok()?;
        if v["media_type"] != "video/v210" {
            return None;
        }
        Some(Video {
            width: v["frame_width"].as_u64()? as u32,
            height: v["frame_height"].as_u64()? as u32,
            rate: (v["grain_rate"]["numerator"].as_u64()? as u32, v["grain_rate"]["denominator"].as_u64().unwrap_or(1) as u32),
            interlaced: v["interlace_mode"].as_str().is_some_and(|m| m.starts_with("interlaced")),
        })
    }
}

/// La description NMOS d'un flux vidéo v210 (ce que MXL attend pour le créer).
pub fn video_flow_def(id: Uuid, label: &str, v: &Video) -> String {
    serde_json::json!({
        "description": label,
        "id": id.to_string(),
        "tags": { "urn:x-nmos:tag:grouphint/v1.0": [format!("Strata:{label}")] },
        "format": "urn:x-nmos:format:video",
        "label": label,
        "parents": [],
        "media_type": "video/v210",
        "grain_rate": { "numerator": v.rate.0, "denominator": v.rate.1.max(1) },
        "frame_width": v.width,
        "frame_height": v.height,
        "interlace_mode": if v.interlaced { "interlaced_tff" } else { "progressive" },
        "colorspace": "BT709",
        "components": [
            { "name": "Y", "width": v.width, "height": v.height, "bit_depth": 10 },
            { "name": "Cb", "width": v.width / 2, "height": v.height, "bit_depth": 10 },
            { "name": "Cr", "width": v.width / 2, "height": v.height, "bit_depth": 10 }
        ]
    })
    .to_string()
}

/// L'écrivain d'un flux vidéo (un seul par flux).
pub struct VideoWriter {
    inst: mxl::MxlInstance,
    grains: mxl::GrainWriter,
    rate: mxl::Rational,
    video: Video,
    last: Option<u64>,
    scratch: Vec<u8>,
}

impl VideoWriter {
    pub fn video(&self) -> Video {
        self.video
    }

    /// L'index du dernier grain écrit (en entrelacé, celui de la dernière trame) : là où poser les
    /// données annexes de cette image (`DataWriter::write_at`, flux de données à la même cadence).
    pub fn last_index(&self) -> Option<u64> {
        self.last
    }

    /// De combien de grains l'écrivain est en avance sur maintenant (MXL ne garde que 200 ms :
    /// trop d'avance, et ce qui est écrit est perdu).
    pub fn ahead(&self) -> i64 {
        let now = self.inst.get_current_index(&self.rate) as i64;
        self.last.map_or(0, |l| l as i64 - now)
    }

    /// L'instant de la dernière image écrite, en échantillons à 48 kHz (sa première trame en
    /// entrelacé) : où poser son son pour qu'il reste calé dessus.
    pub fn last_frame_audio_index(&self) -> Option<u64> {
        let l = self.last?;
        let first = if self.video.interlaced { l.saturating_sub(1) } else { l };
        let (num, den) = (self.rate.numerator.max(1) as u128, self.rate.denominator.max(1) as u128);
        Some((first as u128 * AUDIO_RATE as u128 * den / num) as u64)
    }

    /// L'index du prochain grain : celui de maintenant, jamais un déjà écrit. En entrelacé, une
    /// image commence sur un index pair (trame du haut), sa seconde trame sur l'impair qui suit.
    fn next_index(&mut self, first_field: bool) -> u64 {
        let now = self.inst.get_current_index(&self.rate);
        let mut i = match self.last {
            Some(l) if now <= l => l + 1,
            _ => now,
        };
        if self.video.interlaced {
            match (first_field, self.last) {
                (true, _) if i % 2 == 1 => i += 1,
                // La seconde trame va juste après la première, quelle que soit l'heure d'arrivée
                // (une trame qui finit d'arriver après la frontière tomberait sinon sur un pair).
                (false, Some(l)) if l % 2 == 0 && i > l => i = l + 1,
                (false, _) if i % 2 == 0 => i += 1,
                _ => {}
            }
        }
        self.last = Some(i);
        i
    }

    fn grain(&mut self, first_field: bool, lines: &mut dyn Iterator<Item = &[u8]>) -> Result<(), String> {
        let index = self.next_index(first_field);
        let stride = v210_stride(self.video.width);
        let mut g = self.grains.open_grain(index).map_err(|e| format!("cannot open MXL grain {index}: {e}"))?;
        let slices = g.total_slices();
        let payload = g.payload_mut();
        for (dst, src) in payload.chunks_exact_mut(stride).zip(lines) {
            dst[..src.len().min(stride)].copy_from_slice(&src[..src.len().min(stride)]);
        }
        g.commit(slices).map_err(|e| format!("cannot commit MXL grain {index}: {e}"))
    }

    /// Une image v210 (`stride` octets par ligne) : un grain, ou deux trames en entrelacé.
    pub fn write_v210(&mut self, frame: &[u8], stride: usize) -> Result<(), String> {
        let h = self.video.height as usize;
        let row = |y: usize| &frame[y * stride..(y * stride + stride).min(frame.len())];
        if self.video.interlaced {
            self.grain(true, &mut (0..h).step_by(2).map(row))?;
            self.grain(false, &mut (1..h).step_by(2).map(row))
        } else {
            self.grain(true, &mut (0..h).map(row))
        }
    }

    /// Une trame v210 seule (entrelacé : `second` pour la trame du bas), un grain.
    pub fn write_field_v210(&mut self, field: &[u8], stride: usize, second: bool) -> Result<(), String> {
        let lines = self.video.grain_lines() as usize;
        let row = |y: usize| &field[y * stride..(y * stride + stride).min(field.len())];
        self.grain(!second, &mut (0..lines).map(row))
    }

    /// Une image UYVY 8 bits, passée en v210.
    pub fn write_uyvy(&mut self, frame: &[u8], stride: usize) -> Result<(), String> {
        let (w, h) = (self.video.width as usize, self.video.height as usize);
        let out = v210_stride(self.video.width);
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.resize(out * h, 0);
        for y in 0..h {
            uyvy_to_v210(&frame[y * stride..y * stride + w * 2], &mut scratch[y * out..(y + 1) * out], w);
        }
        let r = self.write_v210(&scratch, out);
        self.scratch = scratch;
        r
    }
}

/// Un flux vidéo suivi par un consommateur (sortie SDI, envoi ST 2110, aperçu) : le lecteur est
/// ouvert quand l'écrivain apparaît, fermé quand il disparaît, et rouvert quand il recrée le flux à
/// un autre format (le flux recréé est un autre fichier : l'ancien lecteur ne verrait plus rien).
/// Vérifié deux fois par seconde au plus (chaque vérification ouvre et verrouille un fichier).
pub struct Follow {
    id: Uuid,
    reader: Option<VideoReader>,
    checked: Option<std::time::Instant>,
}

impl Follow {
    pub fn new(id: Uuid) -> Follow {
        Follow { id, reader: None, checked: None }
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    /// Le lecteur du moment, s'il y a un écrivain.
    pub fn reader(&mut self, mxl: &Mxl) -> Option<&VideoReader> {
        let due = self.checked.is_none_or(|t| t.elapsed() >= Duration::from_millis(500));
        if due {
            self.checked = Some(std::time::Instant::now());
            if !mxl.is_active(self.id) {
                self.reader = None;
            } else {
                if self.reader.as_ref().is_some_and(|r| mxl.video_of(self.id) != Some(r.video)) {
                    self.reader = None;
                }
                if self.reader.is_none() {
                    self.reader = mxl.video_reader(self.id).ok();
                }
            }
        }
        self.reader.as_ref()
    }

    /// Oublie le lecteur (revu au prochain appel).
    pub fn reset(&mut self) {
        self.reader = None;
        self.checked = None;
    }
}

/// Le lecteur d'un flux vidéo.
pub struct VideoReader {
    inst: mxl::MxlInstance,
    grains: mxl::GrainReader,
    rate: mxl::Rational,
    pub video: Video,
    /// Le début de la dernière image lue, en échantillons à 48 kHz (`u64::MAX` : aucune).
    read_at: std::sync::atomic::AtomicU64,
}

impl VideoReader {
    /// L'image la plus récente, lue en place dans MXL (sans copie) : `f(image, None)`, ou en
    /// entrelacé `f(trame du haut, Some(trame du bas))` (chacune `v210_stride(width)` octets par
    /// ligne, `grain_lines()` lignes). None si rien n'a été écrit depuis `fresh`.
    pub fn with_latest<R>(&self, fresh: Duration, f: impl FnOnce(&[u8], Option<&[u8]>) -> R) -> Option<R> {
        let head = self.grains.get_runtime_info().ok()?.headIndex;
        let now = self.inst.get_current_index(&self.rate);
        let age = now.saturating_sub(head) as f64 * self.rate.denominator as f64 / self.rate.numerator.max(1) as f64;
        if age > fresh.as_secs_f64() {
            return None;
        }
        let size = v210_stride(self.video.width) * self.video.grain_lines() as usize;
        // Sans attendre : `get_complete_grain` du crate tourne sans fin sur un grain resté partiel
        // (un écrivain arrêté ou remplacé en plein grain) et bloque qui le lit. Les grains jusqu'à
        // la tête sont validés en entier par l'écrivain.
        let grain = |i: u64| self.grains.get_grain_non_blocking(i).ok().filter(|g| g.index == i && g.payload.len() >= size);
        if !self.video.interlaced {
            let g = grain(head)?;
            self.note_read(head);
            return Some(f(&g.payload[..size], None));
        }
        // Les deux trames d'une même image : la paire (paire, impaire) la plus récente.
        let first = if head % 2 == 1 { head - 1 } else { head.saturating_sub(2) };
        let (a, b) = (grain(first)?, grain(first + 1)?);
        self.note_read(first);
        Some(f(&a.payload[..size], Some(&b.payload[..size])))
    }

    fn note_read(&self, first_grain: u64) {
        let (num, den) = (self.rate.numerator.max(1) as u128, self.rate.denominator.max(1) as u128);
        let at = (first_grain as u128 * AUDIO_RATE as u128 * den / num) as u64;
        self.read_at.store(at, std::sync::atomic::Ordering::Relaxed);
    }

    /// Le début de la dernière image lue (`with_latest`, `latest_into`), en échantillons à 48 kHz :
    /// là où l'appli a posé son son (`VideoWriter::last_frame_audio_index`).
    pub fn last_frame_audio_index(&self) -> Option<u64> {
        Some(self.read_at.load(std::sync::atomic::Ordering::Relaxed)).filter(|v| *v != u64::MAX)
    }

    /// L'image la plus récente copiée dans `out` (réutilisé : pas d'allocation à chaque image),
    /// trames entrelacées remises ensemble. Faux si rien n'a été écrit depuis `fresh`.
    pub fn latest_into(&self, fresh: Duration, out: &mut Vec<u8>) -> bool {
        let stride = v210_stride(self.video.width);
        let lines = self.video.grain_lines() as usize;
        self.with_latest(fresh, |a, b| {
            out.resize(stride * self.video.height as usize, 0);
            match b {
                None => out[..a.len()].copy_from_slice(a),
                Some(b) => {
                    for y in 0..lines {
                        out[(2 * y) * stride..(2 * y + 1) * stride].copy_from_slice(&a[y * stride..(y + 1) * stride]);
                        out[(2 * y + 1) * stride..(2 * y + 2) * stride].copy_from_slice(&b[y * stride..(y + 1) * stride]);
                    }
                }
            }
        })
        .is_some()
    }

    /// L'image la plus récente, dans un nouveau tampon (voir [`Self::latest_into`]).
    pub fn latest(&self, fresh: Duration) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        self.latest_into(fresh, &mut out).then_some(out)
    }

    /// La dernière lecture d'une appli (ns TAI), d'après MXL : quelqu'un lit-il ce flux ?
    pub fn last_read_ns(&self) -> Option<u64> {
        self.grains.get_runtime_info().ok().map(|r| r.lastReadTime).filter(|t| *t > 0)
    }
}

// ---------- Le son ----------

/// Le son des flux de Strata : 48 kHz, float32, un tampon par canal.
pub const AUDIO_RATE: u32 = 48_000;
const AUDIO_RATIONAL: mxl::Rational = mxl::Rational { numerator: AUDIO_RATE as i64, denominator: 1 };

/// La description NMOS d'un flux audio float32 48 kHz.
pub fn audio_flow_def(id: Uuid, label: &str, channels: u32) -> String {
    serde_json::json!({
        "description": label,
        "id": id.to_string(),
        "tags": { "urn:x-nmos:tag:grouphint/v1.0": [format!("Strata:{label}")] },
        "format": "urn:x-nmos:format:audio",
        "label": label,
        "parents": [],
        "media_type": "audio/float32",
        "sample_rate": { "numerator": AUDIO_RATE },
        "channel_count": channels,
        "bit_depth": 32
    })
    .to_string()
}

/// L'écrivain d'un flux audio : des blocs d'échantillons entrelacés, posés à la suite.
pub struct AudioWriter {
    inst: mxl::MxlInstance,
    samples: mxl::SamplesWriter,
    channels: usize,
    /// L'index du prochain échantillon.
    next: Option<u64>,
    max: usize,
}

impl AudioWriter {
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Des échantillons entrelacés posés à partir de l'index `start` (celui d'une image : le son
    /// reste calé sur elle).
    pub fn write_at(&mut self, start: u64, pcm: &[f32]) -> Result<(), String> {
        self.next = Some(start);
        self.write_interleaved(pcm)
    }

    /// Des échantillons entrelacés (`channels` par instant). Posés à la suite des précédents (une
    /// source qui livre son son par paquets écrit un peu en avance : c'est voulu) ; recalés sur
    /// maintenant s'ils s'en écartent de plus d'une seconde (source arrêtée puis reprise).
    pub fn write_interleaved(&mut self, pcm: &[f32]) -> Result<(), String> {
        let n_total = pcm.len() / self.channels.max(1);
        let now = self.inst.get_current_index(&AUDIO_RATIONAL);
        let start = match self.next {
            Some(n) if n.abs_diff(now) < AUDIO_RATE as u64 => n,
            _ => now,
        };
        let mut done = 0;
        while done < n_total {
            let n = (n_total - done).min(self.max);
            let head = start + (done + n) as u64 - 1;
            let mut w = self.samples.open_samples(head, n).map_err(|e| format!("cannot open MXL samples {head}: {e}"))?;
            for c in 0..self.channels.min(w.channels()) {
                let (a, b) = w.channel_data_mut(c).map_err(|e| e.to_string())?;
                let mut k = done;
                for chunk in [a, b] {
                    for dst in chunk.as_chunks_mut::<4>().0 {
                        if k >= done + n {
                            break;
                        }
                        *dst = pcm[k * self.channels + c].to_le_bytes();
                        k += 1;
                    }
                }
            }
            w.commit().map_err(|e| format!("cannot commit MXL samples {head}: {e}"))?;
            done += n;
        }
        self.next = Some(start + n_total as u64);
        Ok(())
    }
}

/// Le lecteur d'un flux audio : des blocs à la suite, un peu derrière l'écrivain.
pub struct AudioReader {
    inst: mxl::MxlInstance,
    samples: mxl::SamplesReader,
    pub channels: usize,
    next: Option<u64>,
}

impl AudioReader {
    /// L'index de l'échantillon de maintenant (horloge TAI).
    pub fn now(&self) -> u64 {
        self.inst.get_current_index(&AUDIO_RATIONAL)
    }

    /// L'index du dernier échantillon écrit.
    pub fn written(&self) -> Option<u64> {
        self.samples.get_runtime_info().ok().map(|r| r.headIndex)
    }

    /// Les `n` échantillons qui commencent à `start` (en échantillons à 48 kHz), entrelacés, dans
    /// `out` (réutilisé). Faux s'ils ne sont pas (encore, ou plus) dans le flux.
    pub fn read_at(&self, start: u64, n: usize, out: &mut Vec<f32>) -> bool {
        let Some(written) = self.samples.get_runtime_info().ok().map(|i| i.headIndex) else { return false };
        // MXL garde 400 ms de son ; au-delà, c'est perdu.
        if n == 0 || start + n as u64 - 1 > written || written - start > AUDIO_RATE as u64 * 2 / 5 {
            return false;
        }
        let Ok(data) = self.samples.get_samples_non_blocking(start + n as u64 - 1, n) else { return false };
        out.clear();
        out.resize(n * self.channels, 0.0);
        for c in 0..self.channels.min(data.num_of_channels()) {
            let Ok((a, b)) = data.channel_data(c) else { return false };
            let mut k = 0;
            for chunk in [a, b] {
                for src in chunk.as_chunks::<4>().0 {
                    if k >= n {
                        break;
                    }
                    out[k * self.channels + c] = f32::from_le_bytes(*src);
                    k += 1;
                }
            }
        }
        true
    }

    pub fn next_interleaved(&mut self, n: usize) -> Option<Vec<f32>> {
        let written = self.samples.get_runtime_info().ok()?.headIndex;
        let margin = AUDIO_RATE as u64 / 25;
        let ideal = written.saturating_sub(margin + n as u64);
        let start = match self.next {
            Some(s) if s.abs_diff(ideal) < AUDIO_RATE as u64 / 10 => s,
            _ => ideal,
        };
        if start + n as u64 - 1 > written {
            self.next = Some(start);
            return None;
        }
        self.next = Some(start + n as u64);
        let head = start + n as u64 - 1;
        let data = self.samples.get_samples_non_blocking(head, n).ok()?;
        let mut out = vec![0f32; n * self.channels];
        for c in 0..self.channels.min(data.num_of_channels()) {
            let (a, b) = data.channel_data(c).ok()?;
            let mut k = 0;
            for chunk in [a, b] {
                for src in chunk.as_chunks::<4>().0 {
                    if k >= n {
                        break;
                    }
                    out[k * self.channels + c] = f32::from_le_bytes(*src);
                    k += 1;
                }
            }
        }
        Some(out)
    }
}

// ---------- v210 ----------

/// Octets par ligne en v210 : 6 pixels par bloc de 16 octets, lignes alignées sur 128 octets.
pub fn v210_stride(width: u32) -> usize {
    (width as usize).div_ceil(48) * 128
}

/// Une ligne UYVY 8 bits (Cb Y Cr Y…) en v210 (10 bits, valeurs × 4).
pub fn uyvy_to_v210(src: &[u8], dst: &mut [u8], width: usize) {
    let blocks = width.div_ceil(6);
    // Les blocs entiers (6 pixels, 12 octets → 16), sans vérification à chaque valeur.
    let full = (width / 6).min(src.len() / 12).min(dst.len() / 16);
    for (i, o) in src.chunks_exact(12).zip(dst.chunks_exact_mut(16)).take(full) {
        for k in 0..4 {
            let w = (u32::from(i[3 * k]) << 2) | (u32::from(i[3 * k + 1]) << 12) | (u32::from(i[3 * k + 2]) << 22);
            o[4 * k..4 * k + 4].copy_from_slice(&w.to_le_bytes());
        }
    }
    // Le dernier bloc entamé : les valeurs au-delà de la ligne à 0.
    let px = |i: usize| -> u32 { u32::from(*src.get(i).unwrap_or(&0)) << 2 };
    for b in full..blocks {
        let base = b * 12;
        let v: [u32; 12] = std::array::from_fn(|k| if (b * 6 + k / 2) < width { px(base + k) } else { 0 });
        let words =
            [v[0] | (v[1] << 10) | (v[2] << 20), v[3] | (v[4] << 10) | (v[5] << 20), v[6] | (v[7] << 10) | (v[8] << 20), v[9] | (v[10] << 10) | (v[11] << 20)];
        for (k, w) in words.iter().enumerate() {
            let o = b * 16 + k * 4;
            if o + 4 <= dst.len() {
                dst[o..o + 4].copy_from_slice(&w.to_le_bytes());
            }
        }
    }
}

/// Une ligne v210 en UYVY 8 bits (aperçus, entrées partagées sans MXL) : en AVX2 quand le
/// processeur l'a, sinon en code ordinaire (même résultat, octet pour octet).
pub fn v210_to_uyvy(src: &[u8], dst: &mut [u8], width: usize) {
    let mut done = 0;
    #[cfg(target_arch = "x86_64")]
    if std::arch::is_x86_feature_detected!("avx2") {
        // SAFETY : AVX2 est là (vérifié juste avant).
        done = unsafe { v210_to_uyvy_avx2(src, dst, width) };
    }
    v210_to_uyvy_from(src, dst, width, done);
}

/// Le code ordinaire, à partir du bloc `from`.
fn v210_to_uyvy_from(src: &[u8], dst: &mut [u8], width: usize, from: usize) {
    let blocks = width.div_ceil(6);
    let full = (width / 6).min(src.len() / 16).min(dst.len() / 12);
    let start = from.min(full);
    for (i, o) in src.chunks_exact(16).zip(dst.chunks_exact_mut(12)).take(full).skip(start) {
        for k in 0..4 {
            let w = u32::from_le_bytes([i[4 * k], i[4 * k + 1], i[4 * k + 2], i[4 * k + 3]]);
            o[3 * k] = (w >> 2) as u8;
            o[3 * k + 1] = (w >> 12) as u8;
            o[3 * k + 2] = (w >> 22) as u8;
        }
    }
    for b in full.max(from)..blocks {
        for k in 0..4 {
            let o = b * 16 + k * 4;
            let Some(w) = src.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])) else { return };
            for j in 0..3 {
                let i = b * 12 + k * 3 + j;
                if i / 2 < width && i < dst.len() {
                    dst[i] = (((w >> (10 * j)) & 0x3ff) >> 2) as u8;
                }
            }
        }
    }
}

/// v210 → UYVY, deux blocs (12 pixels) à la fois ; rend le nombre de blocs faits (le reste : en
/// code ordinaire).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn v210_to_uyvy_avx2(src: &[u8], dst: &mut [u8], width: usize) -> usize {
    use std::arch::x86_64::*;
    let full = (width / 6).min(src.len() / 16).min(dst.len() / 12);
    let pairs = full / 2;
    // Chaque mot de 32 bits : trois valeurs de 10 bits ; on garde leurs 8 bits du haut.
    let ff = _mm256_set1_epi32(0xff);
    // Les 3 octets utiles de chacun des 4 mots d'une moitié, serrés en 12 octets.
    let pack = _mm256_setr_epi8(
        0, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13, 14, -1, -1, -1, -1, //
        0, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13, 14, -1, -1, -1, -1,
    );
    let mut out = [0u8; 32];
    for p in 0..pairs {
        let w = _mm256_loadu_si256(src.as_ptr().add(p * 32) as *const __m256i);
        let a = _mm256_and_si256(_mm256_srli_epi32::<2>(w), ff);
        let b = _mm256_slli_epi32::<8>(_mm256_and_si256(_mm256_srli_epi32::<12>(w), ff));
        let c = _mm256_slli_epi32::<16>(_mm256_and_si256(_mm256_srli_epi32::<22>(w), ff));
        let x = _mm256_shuffle_epi8(_mm256_or_si256(a, _mm256_or_si256(b, c)), pack);
        let o = p * 24;
        if o + 28 <= dst.len() {
            // Deux écritures de 16 octets : les 4 de trop de chacune sont réécrites juste après
            // (par la seconde, puis par le tour suivant).
            _mm_storeu_si128(dst.as_mut_ptr().add(o) as *mut __m128i, _mm256_castsi256_si128(x));
            _mm_storeu_si128(dst.as_mut_ptr().add(o + 12) as *mut __m128i, _mm256_extracti128_si256::<1>(x));
        } else {
            _mm256_storeu_si256(out.as_mut_ptr() as *mut __m256i, x);
            dst[o..o + 12].copy_from_slice(&out[..12]);
            dst[o + 12..o + 24].copy_from_slice(&out[16..28]);
        }
    }
    pairs * 2
}

/// Un flux de données annexes (ANC) : un grain `video/smpte291` (RFC 8331) par image, ou par trame
/// en entrelacé (`rate` est alors la cadence des trames : 50 pour du 1080i50).
pub fn data_flow_def(id: Uuid, label: &str, rate: (u32, u32)) -> String {
    serde_json::json!({
        "description": label,
        "id": id.to_string(),
        "tags": { "urn:x-nmos:tag:grouphint/v1.0": [format!("Strata:{label}")] },
        "format": "urn:x-nmos:format:data",
        "label": label,
        "parents": [],
        "media_type": "video/smpte291",
        "grain_rate": { "numerator": rate.0, "denominator": rate.1.max(1) }
    })
    .to_string()
}

impl Mxl {
    /// Crée (ou reprend) le flux de données `id`, à `rate` grains par seconde.
    pub fn data_writer(&self, id: Uuid, label: &str, rate: (u32, u32)) -> Result<DataWriter, String> {
        // Un ancien flux de ce nom, sans écrivain : effacé (il pouvait être à une autre cadence).
        if !self.is_active(id) {
            self.collect();
        }
        let def = data_flow_def(id, label, rate);
        let (w, info, _) = self.inst.create_flow_writer(&def, None).map_err(|e| format!("cannot create the MXL flow {id}: {e}"))?;
        let actual = info.common().grain_rate().map_err(|e| e.to_string())?;
        if (actual.numerator as u64) * rate.1.max(1) as u64 != (rate.0 as u64) * actual.denominator.max(1) as u64 {
            return Err(format!("the MXL flow {id} is still open at another rate by another writer"));
        }
        let grains = w.to_grain_writer().map_err(|e| e.to_string())?;
        Ok(DataWriter { inst: self.inst.clone(), grains, rate: actual, last: None })
    }

    /// Ouvre le flux de données `id` en lecture.
    pub fn data_reader(&self, id: Uuid) -> Result<DataReader, String> {
        let def = self.inst.get_flow_def(&id.to_string()).map_err(|e| format!("no MXL flow {id}: {e}"))?;
        let v: serde_json::Value = serde_json::from_str(&def).map_err(|e| e.to_string())?;
        if v["media_type"] != "video/smpte291" {
            return Err(format!("the MXL flow {id} is not ancillary data (video/smpte291)"));
        }
        let r = self.inst.create_flow_reader(&id.to_string()).map_err(|e| format!("cannot read the MXL flow {id}: {e}"))?;
        let grains = r.to_grain_reader().map_err(|e| e.to_string())?;
        let rate = grains.get_config_info().map_err(|e| e.to_string())?.common().grain_rate().map_err(|e| e.to_string())?;
        Ok(DataReader { inst: self.inst.clone(), grains, rate, next: None })
    }
}

/// L'écrivain d'un flux de données.
pub struct DataWriter {
    inst: mxl::MxlInstance,
    grains: mxl::GrainWriter,
    rate: mxl::Rational,
    last: Option<u64>,
}

impl DataWriter {
    /// Les grains par seconde.
    pub fn rate(&self) -> (u32, u32) {
        (self.rate.numerator as u32, self.rate.denominator as u32)
    }

    /// Les paquets d'une image (ou d'une trame : `field` 2 ou 3), à l'instant présent.
    pub fn write(&mut self, packets: &[anc::Packet], field: anc::Field) -> Result<(), String> {
        let now = self.inst.get_current_index(&self.rate);
        let index = match self.last {
            Some(l) if now <= l => l + 1,
            _ => now,
        };
        self.write_at(index, packets, field)
    }

    /// Les paquets du grain `index` (celui d'une image : `VideoWriter::last_index`, à la même
    /// cadence).
    pub fn write_at(&mut self, index: u64, packets: &[anc::Packet], field: anc::Field) -> Result<(), String> {
        let payload = anc::encode(packets, field);
        let mut g = self.grains.open_grain(index).map_err(|e| format!("cannot open MXL grain {index}: {e}"))?;
        let slices = g.total_slices();
        let dst = g.payload_mut();
        if payload.len() > dst.len() {
            return Err(format!("too much ancillary data for one grain ({} bytes)", payload.len()));
        }
        dst[..payload.len()].copy_from_slice(&payload);
        dst[payload.len()..].fill(0);
        g.commit(slices).map_err(|e| format!("cannot commit MXL grain {index}: {e}"))?;
        self.last = Some(index);
        Ok(())
    }
}

/// Le lecteur d'un flux de données : les grains dans l'ordre, sans en sauter (sauf s'il prend plus
/// de 3 grains de retard : il repart du dernier écrit).
pub struct DataReader {
    inst: mxl::MxlInstance,
    grains: mxl::GrainReader,
    rate: mxl::Rational,
    next: Option<u64>,
}

impl DataReader {
    /// Les grains par seconde.
    pub fn rate(&self) -> (u32, u32) {
        (self.rate.numerator as u32, self.rate.denominator as u32)
    }

    /// Le grain suivant, s'il est écrit : ses paquets et son champ F. None sinon (rien de neuf,
    /// ou un grain mal formé).
    pub fn next_grain(&mut self) -> Option<(Vec<anc::Packet>, anc::Field)> {
        let head = self.grains.get_runtime_info().ok()?.headIndex;
        // Un écrivain qui ne bouge plus (plus d'une seconde) : rien.
        let now = self.inst.get_current_index(&self.rate);
        if now.saturating_sub(head) as u128 * (self.rate.denominator.max(1) as u128) > self.rate.numerator as u128 {
            return None;
        }
        let i = match self.next {
            Some(n) if n <= head && head - n <= 3 => n,
            Some(n) if n == head + 1 => return None,
            _ => head,
        };
        let g = self.grains.get_grain_non_blocking(i).ok().filter(|g| g.index == i)?;
        self.next = Some(i + 1);
        anc::decode(g.payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v210_fast_paths_match() {
        // Les chemins rapides (blocs entiers, AVX2) donnent le même résultat que le calcul pas à pas.
        let reference = |src: &[u8], width: usize| -> Vec<u8> {
            let mut out = vec![0u8; width * 2];
            for (i, o) in out.iter_mut().enumerate() {
                let (b, r) = (i / 12, i % 12);
                let at = b * 16 + (r / 3) * 4;
                let w = u32::from_le_bytes([src[at], src[at + 1], src[at + 2], src[at + 3]]);
                *o = ((w >> (10 * (r % 3) + 2)) & 0xff) as u8;
            }
            out
        };
        for width in [1920usize, 1280, 720, 7, 13] {
            let uyvy: Vec<u8> = (0..width * 2).map(|i| (i * 37 % 251) as u8).collect();
            let mut v210 = vec![0u8; v210_stride(width as u32)];
            uyvy_to_v210(&uyvy, &mut v210, width);
            assert_eq!(reference(&v210, width), uyvy, "reference, width {width}");
            let mut plain = vec![0u8; width * 2];
            v210_to_uyvy_from(&v210, &mut plain, width, 0);
            assert_eq!(plain, uyvy, "plain, width {width}");
            let mut fast = vec![0u8; width * 2];
            v210_to_uyvy(&v210, &mut fast, width);
            assert_eq!(fast, uyvy, "dispatched, width {width}");
            // Des mots v210 quelconques (bits du bas, 2 bits du haut non nuls).
            let noisy: Vec<u8> = (0..v210.len()).map(|i| (i * 89 % 256) as u8).collect();
            let (mut a, mut b) = (vec![0u8; width * 2], vec![0u8; width * 2]);
            v210_to_uyvy_from(&noisy, &mut a, width, 0);
            v210_to_uyvy(&noisy, &mut b, width);
            assert_eq!(a, reference(&noisy, width), "noisy plain, width {width}");
            assert_eq!(b, reference(&noisy, width), "noisy dispatched, width {width}");
        }
    }

    #[test]
    fn v210_round_trip() {
        for w in [1920usize, 1280, 720, 3840, 10] {
            let src: Vec<u8> = (0..w * 2).map(|i| (i * 7 % 251) as u8).collect();
            let mut v = vec![0u8; v210_stride(w as u32)];
            uyvy_to_v210(&src, &mut v, w);
            let mut back = vec![0u8; w * 2];
            v210_to_uyvy(&v, &mut back, w);
            assert_eq!(src, back, "width {w}");
        }
        assert_eq!(v210_stride(1920), 5120);
        assert_eq!(v210_stride(1280), 3456);
    }

    #[test]
    fn stable_ids_and_flow_definitions() {
        assert_eq!(flow_id("sdi/7B10A04E/1"), flow_id("sdi/7b10a04e/1"));
        assert_ne!(flow_id("sdi/7b10a04e/1"), flow_id("sdi/7b10a04e/2"));
        let v = Video { width: 1920, height: 1080, rate: (25, 1), interlaced: true };
        let def = video_flow_def(flow_id("sdi/x/1"), "SDI 1", &v);
        assert_eq!(Video::from_flow_def(&def), Some(v));
        assert!(def.contains("interlaced_tff") && def.contains("video/v210"));
    }

    /// Avec la vraie bibliothèque (voir plus bas).
    #[test]
    #[ignore]
    fn writes_and_reads_ancillary_data() {
        let dir = std::env::temp_dir().join(format!("strata-mxl-anc-{}", std::process::id()));
        let mxl = Mxl::open_at(&dir).unwrap();
        let id = flow_id("test/anc");
        let mut w = mxl.data_writer(id, "ANC", (50, 1)).unwrap();
        let mut r = mxl.data_reader(id).unwrap();
        assert_eq!(r.rate(), (50, 1));
        assert!(r.next_grain().is_none(), "nothing written yet");
        for f in 0..4u8 {
            w.write(&[anc::atc_packet(1, 2, 3, f, false)], 2 + f % 2).unwrap();
        }
        // Le dernier écrit (le lecteur part de la tête), puis rien de neuf.
        let (ps, field) = r.next_grain().unwrap();
        assert_eq!((anc::atc_timecode(&ps[0]), field), (Some((1, 2, 3, 3, false)), 3));
        assert!(r.next_grain().is_none());
        // Ensuite, dans l'ordre.
        for f in 4..6u8 {
            w.write(&[anc::atc_packet(1, 2, 3, f, false)], 2).unwrap();
        }
        assert_eq!(anc::atc_timecode(&r.next_grain().unwrap().0[0]).unwrap().3, 4);
        assert_eq!(anc::atc_timecode(&r.next_grain().unwrap().0[0]).unwrap().3, 5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Avec la vraie bibliothèque : `STRATA_MXL_LIB=…/libmxl.so cargo test -p strata-mxl -- --ignored`.
    #[test]
    #[ignore]
    fn writes_and_reads_a_real_flow() {
        let dir = std::env::temp_dir().join(format!("strata-mxl-{}", std::process::id()));
        let mxl = Mxl::open_at(&dir).unwrap();
        for interlaced in [false, true] {
            let v = Video { width: 1280, height: 720, rate: (25, 1), interlaced };
            let id = flow_id(&format!("test/{interlaced}"));
            let mut w = mxl.video_writer(id, "test", &v).unwrap();
            let frame: Vec<u8> = (0..1280 * 2 * 720).map(|i| ((i / 2560) % 200) as u8 + 16).collect();
            w.write_uyvy(&frame, 2560).unwrap();
            assert!(mxl.is_active(id));
            let r = mxl.video_reader(id).unwrap();
            assert_eq!(r.video, v);
            let got = r.latest(Duration::from_secs(2)).expect("a picture");
            let stride = v210_stride(1280);
            // Chaque ligne a gardé sa valeur, trames remises en place.
            for y in [0usize, 1, 2, 361, 719] {
                let mut line = vec![0u8; 2560];
                v210_to_uyvy(&got[y * stride..(y + 1) * stride], &mut line, 1280);
                assert_eq!(line[1], ((y % 200) + 16) as u8, "line {y} interlaced {interlaced}");
            }
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    #[ignore]
    fn writes_and_reads_real_audio() {
        let dir = std::env::temp_dir().join(format!("strata-mxl-audio-{}", std::process::id()));
        let mxl = Mxl::open_at(&dir).unwrap();
        let id = flow_id("test/audio");
        let mut w = mxl.audio_writer(id, "test", 2).unwrap();
        let mut r = mxl.audio_reader(id).unwrap();
        assert_eq!(r.channels, 2);
        // 1 s de son, par blocs de 20 ms, à leur rythme : gauche = n, droite = -n.
        let mut n = 0u32;
        let mut got = Vec::new();
        let start = std::time::Instant::now();
        for block in 0..50u32 {
            let pcm: Vec<f32> = (0..960)
                .flat_map(|_| {
                    n += 1;
                    [n as f32, -(n as f32)]
                })
                .collect();
            w.write_interleaved(&pcm).unwrap();
            let due = start + Duration::from_millis(20 * (block as u64 + 1));
            std::thread::sleep(due.saturating_duration_since(std::time::Instant::now()));
            if let Some(b) = r.next_interleaved(960) {
                got.extend(b);
            }
        }
        // Le lecteur suit, 40 ms derrière : des blocs à la suite, sans trou, canaux à leur place.
        assert!(got.len() >= 2 * 960 * 40, "read {} samples", got.len() / 2);
        let first = got.iter().step_by(2).position(|x| *x > 0.0).expect("written samples");
        let mut prev = got[first * 2];
        for k in first + 1..got.len() / 2 {
            assert_eq!(got[k * 2], prev + 1.0, "sample {k}");
            assert_eq!(got[k * 2 + 1], -got[k * 2]);
            prev = got[k * 2];
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
