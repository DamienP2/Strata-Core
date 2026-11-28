//! Les destinations NDI : le Core envoie en NDI un flux MXL du serveur (celui qu'une appli écrit,
//! `ndi-out/<id>`, ou une entrée : SDI, ST 2110, flux réseau), image et son ensemble.
//!
//! - Une destination par fil d'exécution, tenue en accord avec les réglages.
//! - L'image : la plus récente du flux, en UYVY 8 bits ; NDI cadence l'envoi à la cadence du flux.
//!   Sans image depuis une seconde, du noir au format maison (la source NDI reste visible).
//! - Le son suit l'image : les échantillons posés avec elle (comme les flux réseau), tous les
//!   canaux du flux, en float plan par plan ; sans son, rien (NDI n'en exige pas).
//! - Sans bibliothèque NDI, ou une qui ne sait pas envoyer : la destination le dit et réessaie,
//!   rien d'autre n'est bloqué.
use serde::{Deserialize, Serialize};
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

use super::ndi;

/// Une destination NDI (Réglages › NDI › Destinations).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct NdiOutput {
    pub id: String,
    /// Le nom de la source NDI (les récepteurs voient « MACHINE (nom) »).
    pub name: String,
    pub enabled: bool,
    /// Ce qui part : vide, le flux `ndi-out/<id>` qu'une appli écrit ; sinon un flux du serveur
    /// (`sdi/<carte>/<n>`, `2110/<flux>`, `stream-in/<id>`).
    pub source: String,
    /// Les groupes NDI de la source, séparés par des virgules ; vide : le groupe public.
    pub groups: String,
}

impl NdiOutput {
    /// Le flux vidéo envoyé (son son : `<flux>/audio`).
    pub fn flow(&self) -> String {
        if self.source.trim().is_empty() {
            out_flow(&self.id)
        } else {
            self.source.trim().to_string()
        }
    }
}

/// Le flux qu'une appli écrit pour une destination NDI.
pub fn out_flow(id: &str) -> String {
    format!("ndi-out/{id}")
}

/// Vérifie et nettoie les destinations (appelé par `HardwareConfig::check`).
pub fn check(list: &mut [NdiOutput]) -> Result<(), String> {
    let mut ids = std::collections::HashSet::new();
    let mut names = std::collections::HashSet::new();
    for o in list.iter_mut() {
        o.name = o.name.trim().to_string();
        o.source = o.source.trim().to_string();
        o.groups = o.groups.split(',').map(str::trim).filter(|g| !g.is_empty()).collect::<Vec<_>>().join(",");
        if o.id.is_empty() || o.id.len() > 64 || !o.id.chars().all(|c| c.is_ascii_alphanumeric() || "-_".contains(c)) {
            return Err(format!("{}: bad NDI destination id", o.name));
        }
        if !ids.insert(o.id.clone()) {
            return Err(format!("two NDI destinations have the id {}", o.id));
        }
        if o.name.is_empty() {
            return Err("each NDI destination needs a name".into());
        }
        // Un nom NDI : pas de caractère de contrôle, ni de parenthèse (NDI y met la machine).
        if o.name.len() > 100 || o.name.chars().any(|c| c.is_control() || "()".contains(c)) {
            return Err(format!("{}: an NDI name has no brackets and at most 100 characters", o.name));
        }
        if !names.insert(o.name.to_lowercase()) {
            return Err(format!("two NDI destinations are called {}", o.name));
        }
        if o.groups.len() > 200 || !o.groups.chars().all(|c| c.is_ascii_alphanumeric() || "-_ ,.".contains(c)) {
            return Err(format!("{}: NDI groups are names separated by commas", o.name));
        }
        if o.source.len() > 200 || o.source.chars().any(|c| c.is_control()) {
            return Err(format!("{}: bad source", o.name));
        }
    }
    Ok(())
}

#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum State {
    #[default]
    Starting,
    /// Le flux part en NDI.
    Live,
    /// Rien n'arrive : du noir (la source NDI reste là).
    Black,
    Error,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub id: String,
    pub state: State,
    /// « 1080i50 » : ce qui part.
    pub format: Option<String>,
    /// Images envoyées depuis le départ.
    pub frames: u64,
    /// Les récepteurs branchés sur la source.
    pub connections: Option<u32>,
    /// Le son du flux part avec l'image.
    pub audio: bool,
    /// Canaux de son envoyés.
    pub channels: usize,
    pub error: Option<String>,
}

/// Ce que tient une destination en marche (relancée quand ça change).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    pub out: NdiOutput,
    /// Le format du noir quand rien n'arrive (format maison).
    pub idle: Video,
}

/// Les destinations à tenir d'après les réglages.
pub fn wanted(cfg: &super::HardwareConfig) -> Vec<Wanted> {
    let idle = crate::streams::house_video(cfg);
    cfg.ndi_outputs.iter().filter(|o| o.enabled).map(|o| Wanted { out: o.clone(), idle }).collect()
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

#[derive(Default)]
pub struct NdiOutputs {
    running: Mutex<HashMap<String, Running>>,
}

impl NdiOutputs {
    /// Lance les destinations voulues, arrête les autres (sans MXL : aucune).
    pub fn reconcile(&self, mut wanted: Vec<Wanted>, mxl: Option<Arc<Mxl>>) {
        if mxl.is_none() {
            wanted.clear();
        }
        let stopped: Vec<Running> = {
            let mut run = self.running.lock().unwrap();
            let gone: Vec<String> = run.iter().filter(|(_, r)| !wanted.contains(&r.wanted)).map(|(k, _)| k.clone()).collect();
            gone.into_iter().filter_map(|k| run.remove(&k)).collect()
        };
        // Arrêtées d'abord (hors du verrou) : un nom changé libère l'ancienne source NDI.
        for r in stopped {
            tracing::info!("NDI destination {} stopped", r.wanted.out.name);
            r.halt();
        }
        let Some(mxl) = mxl else { return };
        let mut run = self.running.lock().unwrap();
        for w in wanted {
            if run.contains_key(&w.out.id) {
                continue;
            }
            tracing::info!("NDI destination {} started", w.out.name);
            run.insert(w.out.id.clone(), start(w, mxl.clone()));
        }
    }

    pub fn stop_all(&self) {
        self.reconcile(Vec::new(), None);
    }

    pub fn status_of(&self, id: &str) -> Option<Status> {
        self.running.lock().unwrap().get(id).map(|r| r.status.lock().unwrap().clone())
    }
}

impl Drop for NdiOutputs {
    fn drop(&mut self) {
        self.stop_all();
    }
}

fn start(w: Wanted, mxl: Arc<Mxl>) -> Running {
    let status = Arc::new(Mutex::new(Status { id: w.out.id.clone(), ..Status::default() }));
    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let (w, status, stop) = (w.clone(), status.clone(), stop.clone());
        std::thread::Builder::new().name(format!("ndi-out-{}", w.out.id)).spawn(move || run(&w, &mxl, &stop, &status)).ok()
    };
    Running { wanted: w, stop, status, thread }
}

/// Attend `d`, ou moins si on arrête.
fn pause(stop: &AtomicBool, d: Duration) {
    let end = Instant::now() + d;
    while !stop.load(Ordering::Relaxed) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn run(w: &Wanted, mxl: &Mxl, stop: &AtomicBool, status: &Mutex<Status>) {
    let set = |f: &dyn Fn(&mut Status)| f(&mut status.lock().unwrap());
    while !stop.load(Ordering::Relaxed) {
        let sender = match ndi::can_send().and_then(|_| ndi::Sender::open(&w.out.name, &w.out.groups)) {
            Ok(s) => s,
            Err(e) => {
                set(&|s| {
                    s.state = State::Error;
                    s.error = Some(e.clone());
                });
                // La bibliothèque peut être installée entre-temps.
                pause(stop, Duration::from_secs(10));
                continue;
            }
        };
        set(&|s| s.error = None);
        send(w, mxl, &sender, stop, &set);
    }
}

/// Une image noire UYVY.
fn black_uyvy(v: &Video) -> Vec<u8> {
    (0..v.width as usize * 2).map(|i| if i % 2 == 0 { 128 } else { 16 }).collect::<Vec<u8>>().repeat(v.height as usize)
}

/// Le son entrelacé `ch` canaux en plans (un par canal), dans `out` (réutilisé).
fn planar(interleaved: &[f32], ch: usize, out: &mut Vec<f32>) {
    let n = interleaved.len() / ch.max(1);
    out.clear();
    out.resize(n * ch, 0.0);
    for (k, frame) in interleaved.chunks_exact(ch.max(1)).enumerate() {
        for (c, x) in frame.iter().enumerate() {
            out[c * n + k] = *x;
        }
    }
}

/// Change l'état d'une destination.
type Setter<'a> = &'a dyn Fn(&dyn Fn(&mut Status));

/// Envoie jusqu'à l'arrêt, en suivant le format de la source (le noir au format maison).
fn send(w: &Wanted, mxl: &Mxl, sender: &ndi::Sender, stop: &AtomicBool, set: Setter) {
    let flow = w.out.flow();
    let (vid, aid) = (strata_mxl::flow_id(&flow), strata_mxl::flow_id(&crate::streams::audio_of(&flow)));
    let mut reader: Option<strata_mxl::VideoReader> = None;
    let mut areader: Option<strata_mxl::AudioReader> = None;
    let mut fmt = w.idle;
    let mut v210: Vec<u8> = Vec::new();
    let mut uyvy = black_uyvy(&fmt);
    let mut is_black = true;
    let mut last_live = Instant::now() - Duration::from_secs(10);
    let mut audio_at: Option<u64> = None;
    let (mut abuf, mut pbuf): (Vec<f32>, Vec<f32>) = (Vec::new(), Vec::new());
    let mut acc = 0u64;
    let mut frames = 0u64;
    let mut checked = Instant::now() - Duration::from_secs(1);
    let mut tick = Instant::now() - Duration::from_secs(1);
    let mut next = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        // Les flux, revus deux fois par seconde (chaque vérification ouvre un fichier).
        if checked.elapsed() >= Duration::from_millis(500) {
            checked = Instant::now();
            if mxl.is_active(vid) {
                if reader.is_none() {
                    reader = mxl.video_reader(vid).ok();
                }
            } else {
                reader = None;
            }
            if mxl.is_active(aid) {
                if areader.is_none() {
                    areader = mxl.audio_reader(aid).ok();
                }
            } else {
                areader = None;
            }
            // Une autre forme : la source NDI suit (NDI accepte le changement en cours de route).
            let want = reader.as_ref().map(|r| r.video).unwrap_or(w.idle);
            if want != fmt {
                fmt = want;
                uyvy = black_uyvy(&fmt);
                is_black = true;
                acc = 0;
            }
        }
        let period = Duration::from_secs_f64(fmt.rate.1.max(1) as f64 / fmt.rate.0.max(1) as f64);
        let fresh = reader.as_ref().filter(|r| r.video == fmt).is_some_and(|r| r.latest_into(period * 3, &mut v210));
        if fresh {
            let stride = strata_mxl::v210_stride(fmt.width);
            let line = fmt.width as usize * 2;
            for (src, dst) in v210.chunks_exact(stride).zip(uyvy.chunks_exact_mut(line)) {
                strata_mxl::v210_to_uyvy(src, dst, fmt.width as usize);
            }
            is_black = false;
            last_live = Instant::now();
            audio_at = reader.as_ref().and_then(|r| r.last_frame_audio_index()).or(audio_at);
        } else if !is_black && last_live.elapsed() > Duration::from_secs(1) {
            uyvy = black_uyvy(&fmt);
            is_black = true;
        }
        let live = last_live.elapsed() <= Duration::from_secs(1);
        // Le son de cette image (48 000 × durée, au fil des fractions), posé avec elle.
        acc += 48_000 * fmt.rate.1.max(1) as u64;
        let n = (acc / fmt.rate.0.max(1) as u64) as usize;
        acc %= fmt.rate.0.max(1) as u64;
        let mut channels = 0;
        if let Some(r) = areader.as_mut() {
            let ch = r.channels.max(1);
            let got = match audio_at {
                Some(at) => {
                    // Écrit juste après l'image : on lui laisse quelques ms.
                    let mut ok = r.read_at(at, n, &mut abuf);
                    for _ in 0..4 {
                        if ok {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(2));
                        ok = r.read_at(at, n, &mut abuf);
                    }
                    audio_at = Some(at + n as u64);
                    ok
                }
                None => r.next_interleaved(n).map(|s| abuf = s).is_some(),
            };
            if !got {
                abuf.clear();
                abuf.resize(n * ch, 0.0);
            }
            planar(&abuf, ch, &mut pbuf);
            sender.audio(&mut pbuf, ch, n);
            channels = ch;
        }
        // Cadencé par NDI : rend la main à l'heure de l'image suivante.
        sender.video(&uyvy, fmt.width, fmt.height, fmt.width as usize * 2, fmt.rate, fmt.interlaced);
        // Et par nous, sur une échéance absolue (même période : pas d'image perdue), au cas où la
        // bibliothèque ne cadencerait pas ; un retard de plus d'une seconde est oublié.
        next += period;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else if now - next > Duration::from_secs(1) {
            next = now;
        }
        frames += 1;
        if tick.elapsed() >= Duration::from_secs(1) {
            tick = Instant::now();
            let connections = sender.connections();
            let format = super::playout::format_of(&fmt);
            set(&|s| {
                s.state = if live { State::Live } else { State::Black };
                s.format = Some(format.clone());
                s.frames = frames;
                s.connections = connections;
                s.audio = channels > 0;
                s.channels = channels;
                s.error = None;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(id: &str, name: &str) -> NdiOutput {
        NdiOutput { id: id.into(), name: name.into(), enabled: true, ..NdiOutput::default() }
    }

    #[test]
    fn checks_names_ids_and_groups() {
        let mut l = vec![out("prog", " Programme "), out("clean", "Clean")];
        l[1].groups = " regie , plateau,,".into();
        check(&mut l).unwrap();
        assert_eq!(l[0].name, "Programme");
        assert_eq!(l[1].groups, "regie,plateau");
        assert!(check(&mut [out("a", "X"), out("a", "Y")]).is_err(), "same id");
        assert!(check(&mut [out("a", "X"), out("b", "x")]).is_err(), "same name");
        assert!(check(&mut [out("a", "Prog (1)")]).is_err(), "brackets");
        assert!(check(&mut [out("a b", "X")]).is_err(), "bad id");
        assert!(check(&mut [out("a", "")]).is_err(), "no name");
        let mut g = [out("a", "X")];
        g[0].groups = "a;rm".into();
        assert!(check(&mut g).is_err());
    }

    #[test]
    fn flow_of_a_destination() {
        let mut o = out("prog", "Prog");
        assert_eq!(o.flow(), "ndi-out/prog");
        o.source = "sdi/card/1".into();
        assert_eq!(o.flow(), "sdi/card/1");
    }

    #[test]
    fn audio_goes_planar() {
        let mut p = Vec::new();
        planar(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 2, &mut p);
        assert_eq!(p, [1.0, 3.0, 5.0, 2.0, 4.0, 6.0]);
    }

    #[test]
    fn black_is_uyvy() {
        let v = Video { width: 4, height: 2, rate: (25, 1), interlaced: false };
        assert_eq!(black_uyvy(&v), [128, 16, 128, 16, 128, 16, 128, 16].repeat(2));
    }
}
