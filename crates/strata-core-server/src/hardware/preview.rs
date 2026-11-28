//! Les aperçus des réglages (Matériel › Cartes SDI, ST 2110) : une petite image par seconde,
//! seulement pendant qu'on regarde.
//!
//! - **Entrées SDI** : la capture tourne de toute façon pour les applis ; l'aperçu lit le partage.
//! - **Sorties SDI, flux ST 2110 sortants** : l'appli qui envoie publie aussi ce qu'elle envoie
//!   (`strata_share::sdi_out_path`, `flow_path`), seulement quand l'aperçu le demande
//!   (`Reader::watch`, renouvelé à chaque image demandée).
//! - **Flux ST 2110 entrants** : le Core rejoint le groupe multicast pendant qu'on regarde
//!   (`st2110.rs`), et le quitte ensuite.
//!
//! Une image faite sert à tous ceux qui regardent pendant une seconde : dix navigateurs ouverts
//! coûtent autant qu'un. Un aperçu que personne n'a demandé depuis 5 s est fermé (`sweep`).
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use strata_share::Reader;

use super::{capture, st2110};

/// La largeur des aperçus.
pub const WIDTH: u32 = 480;
/// Une image par seconde au plus.
const EVERY: Duration = Duration::from_millis(900);
/// Fermé quand personne ne l'a demandé depuis.
const IDLE: Duration = Duration::from_secs(5);

/// D'où vient l'image.
pub enum Feed {
    /// Un partage `strata-share` (entrée SDI captée, ou sortie publiée par une appli).
    Share { path: PathBuf, reader: Option<Reader> },
    /// Un flux ST 2110 reçu pour l'occasion.
    Flow(st2110::Receiver),
    /// Un flux MXL (entrée du Core, ou sortie écrite par une appli).
    Mxl { mxl: Arc<strata_mxl::Mxl>, id: strata_mxl::Uuid, reader: Option<strata_mxl::VideoReader> },
    /// Une source NDI reçue pour l'occasion (flux de basse qualité).
    Ndi(Option<super::ndi::Receiver>, String),
}

impl Feed {
    pub fn share(path: PathBuf) -> Feed {
        Feed::Share { path, reader: None }
    }
}

struct Entry {
    asked: Instant,
    made: Option<(Instant, Arc<Vec<u8>>)>,
    feed: Feed,
}

#[derive(Default)]
pub struct Previews {
    entries: Mutex<HashMap<String, Arc<Mutex<Entry>>>>,
}

impl Previews {
    /// L'aperçu `key` (bloquant : jusqu'à 400 ms la première fois, le temps qu'une sortie publie).
    pub fn jpeg(&self, key: &str, make: impl FnOnce() -> Feed) -> Result<Arc<Vec<u8>>, String> {
        let entry = {
            let mut map = self.entries.lock().unwrap();
            map.entry(key.to_string()).or_insert_with(|| Arc::new(Mutex::new(Entry { asked: Instant::now(), made: None, feed: make() }))).clone()
        };
        // Une panique d'une requête précédente n'empêche pas les suivantes.
        let mut e = entry.lock().unwrap_or_else(|p| p.into_inner());
        e.asked = Instant::now();
        if let Some((at, jpeg)) = &e.made {
            if at.elapsed() < EVERY {
                return Ok(jpeg.clone());
            }
        }
        let first = e.made.is_none();
        let fresh = match &mut e.feed {
            Feed::Share { path, reader } => from_share(path, reader),
            Feed::Flow(rx) => match rx.picture() {
                Some(p) => capture::jpeg(&p.data, p.width, p.height, p.width as usize * 2, WIDTH).map(Some),
                None => Ok(None),
            },
            Feed::Mxl { mxl, id, reader } => from_mxl(mxl, *id, reader),
            Feed::Ndi(rx, source) => {
                if rx.is_none() {
                    *rx = Some(super::ndi::Receiver::open(source)?);
                }
                // La première fois, le temps que la source réponde.
                let wait = if first { 1500 } else { 100 };
                match rx.as_mut().and_then(|r| r.latest(wait)) {
                    Some((w, h, px)) => super::ndi::jpeg_rgbx(px, w, h, WIDTH).map(Some),
                    None => Ok(None),
                }
            }
        };
        match fresh {
            Ok(Some(jpeg)) => {
                let jpeg = Arc::new(jpeg);
                e.made = Some((Instant::now(), jpeg.clone()));
                Ok(jpeg)
            }
            // Rien de nouveau : la dernière image, pas trop vieille.
            Ok(None) => match &e.made {
                Some((at, jpeg)) if at.elapsed() < IDLE => Ok(jpeg.clone()),
                _ => Err("no picture yet".into()),
            },
            Err(err) => {
                e.made = None;
                Err(err)
            }
        }
    }

    /// L'état de la réception d'un flux entrant, s'il est regardé.
    pub fn flow_status(&self, key: &str) -> Option<st2110::Status> {
        let e = self.entries.lock().unwrap().get(key).cloned()?;
        let e = e.lock().unwrap_or_else(|p| p.into_inner());
        match &e.feed {
            Feed::Flow(rx) => Some(rx.status()),
            Feed::Share { .. } | Feed::Mxl { .. } | Feed::Ndi(..) => None,
        }
    }

    /// Ferme les aperçus que personne ne regarde plus (une réception ST 2110 quitte son groupe).
    pub fn sweep(&self) {
        let idle: Vec<Arc<Mutex<Entry>>> = {
            let mut map = self.entries.lock().unwrap();
            let keys: Vec<String> = map
                .iter()
                .filter(|(_, e)| match e.try_lock() {
                    Ok(e) => e.asked.elapsed() > IDLE,
                    Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner().asked.elapsed() > IDLE,
                    Err(std::sync::TryLockError::WouldBlock) => false,
                })
                .map(|(k, _)| k.clone())
                .collect();
            keys.iter().filter_map(|k| map.remove(k)).collect()
        };
        // Hors du verrou : arrêter une réception attend la fin de son fil.
        drop(idle);
    }

    #[cfg(test)]
    pub fn keys(&self) -> Vec<String> {
        self.entries.lock().unwrap().keys().cloned().collect()
    }
}

/// L'image la plus récente d'un flux MXL (écrite dans les 2 dernières secondes).
fn from_mxl(mxl: &strata_mxl::Mxl, id: strata_mxl::Uuid, reader: &mut Option<strata_mxl::VideoReader>) -> Result<Option<Vec<u8>>, String> {
    if !mxl.is_active(id) {
        *reader = None;
        return Err("nothing is published here".into());
    }
    // Recréé à un autre format : rouvert (l'ancien lecteur ne verrait plus rien).
    if reader.as_ref().is_some_and(|r| mxl.video_of(id) != Some(r.video) || mxl.stale(id, r.stamp)) {
        *reader = None;
    }
    if reader.is_none() {
        *reader = Some(mxl.video_reader(id)?);
    }
    let Some(r) = reader.as_ref() else { return Ok(None) };
    let (w, h) = (r.video.width as usize, r.video.height as usize);
    let stride = strata_mxl::v210_stride(r.video.width);
    // Lu sur place, sans copier l'image (22 Mo en UHD) : seules les lignes de l'aperçu (une sur
    // `step`) sont converties.
    let step = capture::jpeg_step(w as u32, WIDTH) as usize;
    let rows = h / step;
    let mut uyvy = vec![0u8; w * 2 * rows];
    let done = r.with_latest(Duration::from_secs(2), |a, b| {
        for oy in 0..rows {
            let y = oy * step;
            // Entrelacé : les lignes paires dans la première trame, les impaires dans la seconde.
            let (field, line) = match b {
                Some(b) if y % 2 == 1 => (b, y / 2),
                Some(_) => (a, y / 2),
                None => (a, y),
            };
            if let Some(src) = field.get(line * stride..(line + 1) * stride) {
                strata_mxl::v210_to_uyvy(src, &mut uyvy[oy * w * 2..(oy + 1) * w * 2], w);
            }
        }
    });
    if done.is_none() {
        return Ok(None);
    }
    capture::jpeg_of_rows(&uyvy, w as u32, rows as u32, w * 2, WIDTH).map(Some)
}

/// Une image du partage, si une nouvelle est là (ou arrive dans les 400 ms).
fn from_share(path: &std::path::Path, reader: &mut Option<Reader>) -> Result<Option<Vec<u8>>, String> {
    if reader.as_ref().is_none_or(|r| r.gone()) {
        *reader = None;
        match Reader::observe(path) {
            Ok(r) => *reader = Some(r),
            Err(_) => return Err("nothing is published here".into()),
        }
    }
    let r = reader.as_mut().unwrap();
    // Une sortie n'est publiée que tant qu'on la demande.
    r.watch(IDLE);
    let mut buf = Vec::new();
    let mut f = r.latest(&mut buf);
    if f.is_none() && r.wait(Duration::from_millis(400)) {
        f = r.latest(&mut buf);
    }
    match f {
        Some(f) => capture::jpeg(&buf, f.info.width, f.info.height, f.info.stride as usize, WIDTH).map(Some),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_share::{Format, FrameInfo, Writer};

    #[test]
    fn one_picture_per_second_whoever_asks_and_outputs_only_when_watched() {
        let dir = std::env::temp_dir().join(format!("strata-preview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out");
        let mut w = Writer::create(&path, 2, 64 * 32 * 2).unwrap();
        let info = FrameInfo { width: 64, height: 32, stride: 128, format: Format::Uyvy, rate: (25, 1), interlaced: false, timecode: None };
        // L'appli de sortie : publie seulement quand on regarde.
        let sent = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let app = {
            let (stop, sent) = (stop.clone(), sent.clone());
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    if w.watched() {
                        // Compté avant d'être publié : l'aperçu peut lire l'image aussitôt.
                        sent.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        w.publish(&info, &vec![128u8; 64 * 32 * 2]).unwrap();
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            })
        };
        std::thread::sleep(Duration::from_millis(150));
        // Personne ne regarde : rien n'est publié.
        assert_eq!(sent.load(std::sync::atomic::Ordering::Relaxed), 0);
        let p = Previews::default();
        let a = p.jpeg("out/x/1", || Feed::share(path.clone())).unwrap();
        // Un second regard dans la seconde : la même image, pas une nouvelle.
        let b = p.jpeg("out/x/1", || unreachable!()).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(&a[..2], &[0xFF, 0xD8]);
        assert_eq!(p.keys(), vec!["out/x/1".to_string()]);
        assert!(sent.load(std::sync::atomic::Ordering::Relaxed) > 0);
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        app.join().unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }
}
