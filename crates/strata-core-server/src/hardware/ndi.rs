//! NDI (Réglages › Matériel › NDI) : la bibliothèque NDI de la machine, les réglages réseau que
//! toutes les applis partagent (`ndi-config.v1.json`, lu par chaque programme qui charge NDI), et
//! les sources visibles sur le réseau. Repris de CG (`cg-output/src/ndi.rs`, `cg-server/src/hardware.rs`).
//!
//! Le SDK NDI est distribué par Vizrt sous sa propre licence : il n'est pas lié à la construction,
//! la bibliothèque (`libndi.so.6`, installée par l'installateur de Strata ou le SDK) est chargée à
//! la première demande.
use serde_json::{json, Map, Value};
use std::{
    ffi::{c_char, c_void, CStr},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use super::{net::interface_ip, NdiSettings};

/// Où le Core garde les réglages de NDI (`NDI_CONFIG_DIR` du Core et des applis qu'il lance).
pub fn config_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("ndi")
}

/// Les réglages pour la bibliothèque : NDI sur sa seule interface, le serveur de découverte s'il y
/// en a un, le nom de machine des sources.
pub fn settings(s: &NdiSettings, interface: &str) -> Value {
    let mut ndi = Map::new();
    if let Some(ip) = interface_ip(interface) {
        ndi.insert("adapters".into(), json!({ "allowed": [ip.to_string()] }));
    }
    let discovery = s.discovery.trim();
    if !discovery.is_empty() {
        ndi.insert("networks".into(), json!({ "ips": "", "discovery": discovery }));
    }
    // À la place du nom de l'ordinateur : les récepteurs voient « NOM (source) ».
    let machine = s.machine.trim();
    if !machine.is_empty() {
        ndi.insert("machinename".into(), machine.into());
    }
    json!({ "ndi": ndi })
}

/// Écrit `ndi-config.v1.json` (vrai s'il a changé : les programmes qui ont déjà chargé NDI ne le
/// relisent qu'en redémarrant).
pub fn write_settings(data_dir: &Path, s: &NdiSettings, interface: &str) -> bool {
    let dir = config_dir(data_dir);
    let file = dir.join("ndi-config.v1.json");
    let text = serde_json::to_string_pretty(&settings(s, interface)).unwrap_or_default();
    if std::fs::read_to_string(&file).ok().as_deref() == Some(text.as_str()) {
        return false;
    }
    // D'un coup : une appli qui charge NDI à ce moment ne lit jamais un fichier à moitié écrit.
    if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| strata_core::files::write_atomic(&file, text.as_bytes(), false)) {
        tracing::warn!("cannot write the NDI settings ({}): {e}", file.display());
        return false;
    }
    true
}

/// Les endroits où chercher la bibliothèque.
fn candidates() -> Vec<String> {
    let mut c: Vec<String> = Vec::new();
    c.extend(std::env::var("NDI_LIB_PATH"));
    // Celle installée à côté du Core.
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf())) {
        for n in ["lib/libndi.so.6", "libndi.so.6"] {
            c.push(dir.join(n).to_string_lossy().into_owned());
        }
    }
    for v in ["NDI_RUNTIME_DIR_V6", "NDI_RUNTIME_DIR_V5"] {
        if let Ok(dir) = std::env::var(v) {
            c.push(format!("{dir}/libndi.so.6"));
            c.push(format!("{dir}/libndi.so.5"));
        }
    }
    c.extend(["libndi.so.6", "libndi.so.5", "libndi.so", "/usr/local/lib/libndi.so.6", "/usr/local/lib/libndi.so"].map(String::from));
    let mut seen = std::collections::HashSet::new();
    c.retain(|p| seen.insert(p.clone()));
    c
}

type Instance = *mut c_void;

#[repr(C)]
struct Source {
    p_ndi_name: *const c_char,
    p_url_address: *const c_char,
}

#[repr(C)]
struct FindCreate {
    show_local_sources: bool,
    p_groups: *const c_char,
    p_extra_ips: *const c_char,
}

#[repr(C)]
struct RecvCreate {
    source: Source,
    color_format: i32,
    bandwidth: i32,
    allow_video_fields: bool,
    p_ndi_recv_name: *const c_char,
}

/// `NDIlib_video_frame_v2_t`.
#[repr(C)]
struct VideoFrame {
    xres: i32,
    yres: i32,
    fourcc: u32,
    frame_rate_n: i32,
    frame_rate_d: i32,
    aspect: f32,
    frame_format: i32,
    timecode: i64,
    p_data: *mut u8,
    line_stride: i32,
    p_metadata: *const c_char,
    timestamp: i64,
}

struct Lib {
    _lib: libloading::Library,
    path: String,
    version: Option<String>,
    find_create: unsafe extern "C" fn(*const FindCreate) -> Instance,
    find_wait: unsafe extern "C" fn(Instance, u32) -> bool,
    find_sources: unsafe extern "C" fn(Instance, *mut u32) -> *const Source,
    recv: Option<RecvFns>,
    send: Option<SendFns>,
}

/// L'envoi (destinations NDI) : facultatif aussi.
struct SendFns {
    create: unsafe extern "C" fn(*const SendCreate) -> Instance,
    video: unsafe extern "C" fn(Instance, *const VideoFrame),
    /// `NDIlib_send_send_audio_v3` (NDI 5 et 6).
    audio: Option<unsafe extern "C" fn(Instance, *const AudioFrameV3)>,
    connections: Option<unsafe extern "C" fn(Instance, u32) -> i32>,
    destroy: unsafe extern "C" fn(Instance),
}

/// `NDIlib_send_create_t`.
#[repr(C)]
struct SendCreate {
    p_ndi_name: *const c_char,
    p_groups: *const c_char,
    clock_video: bool,
    clock_audio: bool,
}

/// `NDIlib_audio_frame_v3_t`.
#[repr(C)]
struct AudioFrameV3 {
    sample_rate: i32,
    no_channels: i32,
    no_samples: i32,
    timecode: i64,
    fourcc: u32,
    p_data: *mut u8,
    channel_stride_in_bytes: i32,
    p_metadata: *const c_char,
    timestamp: i64,
}

const fn fourcc(s: &[u8; 4]) -> u32 {
    s[0] as u32 | (s[1] as u32) << 8 | (s[2] as u32) << 16 | (s[3] as u32) << 24
}
const FOURCC_UYVY: u32 = fourcc(b"UYVY");
const FOURCC_FLTP: u32 = fourcc(b"FLTP");
/// NDI fabrique le timecode.
const TIMECODE_SYNTHESIZE: i64 = i64::MAX;

/// La réception (aperçus) : facultative, une bibliothèque sans elle garde la recherche.
struct RecvFns {
    create: unsafe extern "C" fn(*const RecvCreate) -> Instance,
    capture: unsafe extern "C" fn(Instance, *mut VideoFrame, *mut c_void, *mut c_void, u32) -> i32,
    free_video: unsafe extern "C" fn(Instance, *const VideoFrame),
    destroy: unsafe extern "C" fn(Instance),
}

// Les fonctions de la bibliothèque s'appellent de n'importe quel fil.
unsafe impl Send for Lib {}
unsafe impl Sync for Lib {}

fn load() -> Result<Lib, String> {
    let tried = candidates();
    let mut failures = Vec::new();
    for path in &tried {
        let bare = !path.contains('/');
        // SAFETY : charger la bibliothèque NDI lance ses initialisations, ce que le SDK attend.
        let lib = match unsafe { libloading::Library::new(path) } {
            Ok(l) => l,
            Err(e) => {
                if !bare && Path::new(path).exists() {
                    failures.push(format!("{path}: {e}"));
                }
                continue;
            }
        };
        // SAFETY : les signatures sont celles du SDK NDI 5/6.
        unsafe {
            let why = |e: libloading::Error| format!("{path}: {e}");
            let init: libloading::Symbol<unsafe extern "C" fn() -> bool> = lib.get(b"NDIlib_initialize\0").map_err(why)?;
            if !init() {
                return Err("the NDI library refused to start (unsupported processor?)".into());
            }
            let version = lib
                .get::<unsafe extern "C" fn() -> *const c_char>(b"NDIlib_version\0")
                .ok()
                .map(|f| f())
                .filter(|p| !p.is_null())
                .map(|p| CStr::from_ptr(p).to_string_lossy().into_owned());
            let l = Lib {
                find_create: *lib.get(b"NDIlib_find_create_v2\0").map_err(why)?,
                find_wait: *lib.get(b"NDIlib_find_wait_for_sources\0").map_err(why)?,
                find_sources: *lib.get(b"NDIlib_find_get_current_sources\0").map_err(why)?,
                recv: (|| {
                    Some(RecvFns {
                        create: *lib.get(b"NDIlib_recv_create_v3\0").ok()?,
                        capture: *lib.get(b"NDIlib_recv_capture_v2\0").ok()?,
                        free_video: *lib.get(b"NDIlib_recv_free_video_v2\0").ok()?,
                        destroy: *lib.get(b"NDIlib_recv_destroy\0").ok()?,
                    })
                })(),
                send: (|| {
                    Some(SendFns {
                        create: *lib.get(b"NDIlib_send_create\0").ok()?,
                        video: *lib.get(b"NDIlib_send_send_video_v2\0").ok()?,
                        audio: lib.get(b"NDIlib_send_send_audio_v3\0").ok().map(|f| *f),
                        connections: lib.get(b"NDIlib_send_get_no_connections\0").ok().map(|f| *f),
                        destroy: *lib.get(b"NDIlib_send_destroy\0").ok()?,
                    })
                })(),
                path: path.clone(),
                version,
                _lib: lib,
            };
            tracing::info!(path, "NDI library loaded");
            return Ok(l);
        }
    }
    if !failures.is_empty() {
        return Err(format!("the NDI library is there but does not load ({})", failures.join("; ")));
    }
    Err("the NDI library is not installed".into())
}

/// La bibliothèque, chargée une fois trouvée (cherchée de nouveau tant qu'elle ne l'est pas : on
/// peut l'installer entre-temps, au plus toutes les 30 s).
fn lib() -> Result<&'static Lib, String> {
    static LIB: OnceLock<Lib> = OnceLock::new();
    static LAST: Mutex<Option<(Instant, String)>> = Mutex::new(None);
    if let Some(l) = LIB.get() {
        return Ok(l);
    }
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(l) = LIB.get() {
        return Ok(l);
    }
    if let Some((at, why)) = last.as_ref() {
        if at.elapsed() < Duration::from_secs(30) {
            return Err(why.clone());
        }
    }
    match load() {
        Ok(l) => Ok(LIB.get_or_init(|| l)),
        Err(e) => {
            *last = Some((Instant::now(), e.clone()));
            Err(e)
        }
    }
}

/// La bibliothèque chargée : son chemin et sa version ; sinon pourquoi.
pub fn runtime() -> Result<(String, Option<String>), String> {
    lib().map(|l| (l.path.clone(), l.version.clone()))
}

/// Le chercheur de sources du Core, gardé : NDI annonce ses sources en quelques secondes (mDNS,
/// serveur de découverte), un chercheur neuf à chaque recherche ne verrait que les premières.
struct Finder(Instance);
// Utilisé sous le verrou seulement.
unsafe impl Send for Finder {}
static FINDER: Mutex<Option<Finder>> = Mutex::new(None);

/// Les sources NDI vues sur le réseau. La première recherche attend (au plus `wait_ms`) que la
/// liste ne bouge plus pendant une seconde ; les suivantes relisent la liste tenue à jour.
pub fn sources(wait_ms: u32) -> Result<Vec<String>, String> {
    let r = lib()?;
    let mut finder = FINDER.lock().unwrap_or_else(|e| e.into_inner());
    let fresh = finder.is_none();
    if fresh {
        let create = FindCreate { show_local_sources: true, p_groups: std::ptr::null(), p_extra_ips: std::ptr::null() };
        // SAFETY : appel C simple ; l'instance vit autant que le programme.
        let find = unsafe { (r.find_create)(&create) };
        if find.is_null() {
            return Err("NDI cannot look for sources".into());
        }
        *finder = Some(Finder(find));
    }
    let find = finder.as_ref().expect("finder").0;
    let start = Instant::now();
    let budget = Duration::from_millis(if fresh { wait_ms.max(1000) } else { 1500 } as u64);
    let mut quiet = if fresh { wait_ms.max(1000) } else { 400 };
    loop {
        let left = budget.saturating_sub(start.elapsed()).as_millis() as u32;
        if left == 0 {
            break;
        }
        // SAFETY : le chercheur est vivant (jamais détruit).
        let changed = unsafe { (r.find_wait)(find, quiet.min(left)) };
        if !changed {
            break;
        }
        quiet = 1000;
    }
    let mut n = 0u32;
    let mut out = Vec::new();
    // SAFETY : la liste vaut jusqu'au prochain appel sur ce chercheur (sous le verrou).
    unsafe {
        let list = (r.find_sources)(find, &mut n);
        for i in 0..n as usize {
            let s = &*list.add(i);
            if !s.p_ndi_name.is_null() {
                out.push(CStr::from_ptr(s.p_ndi_name).to_string_lossy().into_owned());
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Un récepteur NDI pour l'aperçu d'une source : le flux de plus basse qualité que la source
/// propose (le « proxy » de NDI), en RGBX.
pub struct Receiver {
    inst: Instance,
    _name: std::ffi::CString,
    last: Option<(u32, u32, Vec<u8>)>,
}

// Utilisé sous le verrou de son aperçu seulement.
unsafe impl Send for Receiver {}

impl Receiver {
    pub fn open(source: &str) -> Result<Receiver, String> {
        let r = lib()?;
        let f = r.recv.as_ref().ok_or("this NDI library cannot receive")?;
        let name = std::ffi::CString::new(source).map_err(|_| "bad source name")?;
        let create = RecvCreate {
            source: Source { p_ndi_name: name.as_ptr(), p_url_address: std::ptr::null() },
            // RGBX_RGBA, la plus basse qualité, images entières.
            color_format: 2,
            bandwidth: 0,
            allow_video_fields: false,
            p_ndi_recv_name: c"Strata Core preview".as_ptr(),
        };
        // SAFETY : la structure vit le temps de l'appel ; le nom reste à nous.
        let inst = unsafe { (f.create)(&create) };
        if inst.is_null() {
            return Err("NDI cannot receive this source".into());
        }
        Ok(Receiver { inst, _name: name, last: None })
    }

    /// La dernière image reçue (vide la file) : (largeur, hauteur, RGBX). None si rien de neuf.
    pub fn latest(&mut self, wait_ms: u32) -> Option<(u32, u32, &[u8])> {
        let f = lib().ok()?.recv.as_ref()?;
        let mut got = false;
        let mut wait = wait_ms;
        loop {
            // SAFETY : une image remplie par NDI, rendue ci-dessous.
            let mut v: VideoFrame = unsafe { std::mem::zeroed() };
            let kind = unsafe { (f.capture)(self.inst, &mut v, std::ptr::null_mut(), std::ptr::null_mut(), wait) };
            // 1 : une image vidéo.
            if kind != 1 {
                break;
            }
            let (w, h, stride) = (v.xres.max(0) as u32, v.yres.max(0) as u32, v.line_stride.max(0) as usize);
            if w > 0 && h > 0 && !v.p_data.is_null() && stride >= w as usize * 4 {
                // SAFETY : `stride × h` octets, valides jusqu'à free_video.
                let data = unsafe { std::slice::from_raw_parts(v.p_data, stride * h as usize) };
                let mut px = Vec::with_capacity((w * h * 4) as usize);
                for y in 0..h as usize {
                    px.extend_from_slice(&data[y * stride..y * stride + w as usize * 4]);
                }
                self.last = Some((w, h, px));
                got = true;
            }
            unsafe { (f.free_video)(self.inst, &v) };
            wait = 0;
        }
        if !got {
            return None;
        }
        self.last.as_ref().map(|(w, h, p)| (*w, *h, p.as_slice()))
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        if let Some(f) = lib().ok().and_then(|l| l.recv.as_ref()) {
            // SAFETY : créé par `open`, détruit une fois.
            unsafe { (f.destroy)(self.inst) };
        }
    }
}

/// Une source NDI envoyée par le Core (une destination NDI). NDI cadence l'image (`clock_video`) :
/// chaque envoi attend son tour, à la cadence annoncée.
pub struct Sender {
    inst: Instance,
    _name: std::ffi::CString,
    _groups: std::ffi::CString,
}

// Utilisé par le fil de sa destination seulement.
unsafe impl Send for Sender {}

/// L'envoi est possible : la bibliothèque est là et sait envoyer.
pub fn can_send() -> Result<(), String> {
    lib()?.send.as_ref().map(|_| ()).ok_or_else(|| "this NDI library cannot send".to_string())
}

impl Sender {
    /// `name` : le nom de la source (les récepteurs voient « MACHINE (name) ») ; `groups` : ses
    /// groupes NDI, séparés par des virgules (vide : le groupe public).
    pub fn open(name: &str, groups: &str) -> Result<Sender, String> {
        let r = lib()?;
        let f = r.send.as_ref().ok_or("this NDI library cannot send")?;
        let n = std::ffi::CString::new(name).map_err(|_| "bad NDI name")?;
        let g = std::ffi::CString::new(groups.trim()).map_err(|_| "bad NDI groups")?;
        let create = SendCreate {
            p_ndi_name: n.as_ptr(),
            p_groups: if groups.trim().is_empty() { std::ptr::null() } else { g.as_ptr() },
            clock_video: true,
            clock_audio: false,
        };
        // SAFETY : la structure et les chaînes vivent le temps de l'appel ; NDI les copie.
        let inst = unsafe { (f.create)(&create) };
        if inst.is_null() {
            return Err("NDI cannot create this source (name already used?)".into());
        }
        Ok(Sender { inst, _name: n, _groups: g })
    }

    /// Une image UYVY 8 bits (`stride` octets par ligne) ; en entrelacé, les deux trames
    /// entrelacées dans l'image ; `rate` : images par seconde.
    pub fn video(&self, uyvy: &[u8], width: u32, height: u32, stride: usize, rate: (u32, u32), interlaced: bool) {
        let Some(f) = lib().ok().and_then(|r| r.send.as_ref()) else { return };
        if uyvy.len() < stride * height as usize {
            return;
        }
        let frame = VideoFrame {
            xres: width as i32,
            yres: height as i32,
            fourcc: FOURCC_UYVY,
            frame_rate_n: rate.0 as i32,
            frame_rate_d: rate.1.max(1) as i32,
            aspect: width as f32 / height.max(1) as f32,
            // 0 : entrelacé (deux trames dans l'image), 1 : progressif.
            frame_format: if interlaced { 0 } else { 1 },
            timecode: TIMECODE_SYNTHESIZE,
            p_data: uyvy.as_ptr() as *mut u8,
            line_stride: stride as i32,
            p_metadata: std::ptr::null(),
            timestamp: 0,
        };
        // SAFETY : l'image vit le temps de l'appel (envoi synchrone, `send_video_v2`).
        unsafe { (f.video)(self.inst, &frame) };
    }

    /// Du son 48 kHz, un plan par canal (`channels` plans de `samples` float).
    pub fn audio(&self, planar: &mut [f32], channels: usize, samples: usize) {
        let Some(send) = lib().ok().and_then(|r| r.send.as_ref()).and_then(|f| f.audio) else { return };
        if channels == 0 || samples == 0 || planar.len() < channels * samples {
            return;
        }
        let frame = AudioFrameV3 {
            sample_rate: 48_000,
            no_channels: channels as i32,
            no_samples: samples as i32,
            timecode: TIMECODE_SYNTHESIZE,
            fourcc: FOURCC_FLTP,
            p_data: planar.as_mut_ptr() as *mut u8,
            channel_stride_in_bytes: (samples * 4) as i32,
            p_metadata: std::ptr::null(),
            timestamp: 0,
        };
        // SAFETY : le son vit le temps de l'appel (NDI le copie).
        unsafe { send(self.inst, &frame) };
    }

    /// Les récepteurs branchés sur cette source.
    pub fn connections(&self) -> Option<u32> {
        let f = lib().ok()?.send.as_ref()?.connections?;
        // SAFETY : instance vivante ; pas d'attente.
        Some(unsafe { f(self.inst, 0) }.max(0) as u32)
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        if let Some(f) = lib().ok().and_then(|r| r.send.as_ref()) {
            // SAFETY : créée par `open`, détruite une fois.
            unsafe { (f.destroy)(self.inst) };
        }
    }
}

/// Un JPEG de `width` de large au plus, depuis du RGBX.
pub fn jpeg_rgbx(px: &[u8], w: u32, h: u32, width: u32) -> Result<Vec<u8>, String> {
    let step = w.div_ceil(width.max(16)).max(1);
    let (ow, oh) = (w / step, h / step);
    let mut rgb = Vec::with_capacity((ow * oh * 3) as usize);
    for oy in 0..oh {
        for ox in 0..ow {
            let p = ((oy * step * w + ox * step) * 4) as usize;
            rgb.extend_from_slice(&px[p..p + 3]);
        }
    }
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, 72).encode(&rgb, ow as u16, oh as u16, jpeg_encoder::ColorType::Rgb).map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_follow_the_roles() {
        let s = NdiSettings { discovery: "10.0.0.2".into(), machine: String::new() };
        // Pas d'interface de ce nom : pas de restriction.
        let v = settings(&s, "no-such-nic");
        assert_eq!(v["ndi"]["networks"]["discovery"], "10.0.0.2");
        assert!(v["ndi"].get("adapters").is_none());
        let lo = if_addrs::get_if_addrs().unwrap().into_iter().find(|i| i.is_loopback() && i.ip().is_ipv4()).unwrap();
        let v = settings(&NdiSettings::default(), &lo.name);
        assert_eq!(v["ndi"]["adapters"]["allowed"][0], "127.0.0.1");
        assert!(v["ndi"].get("networks").is_none());
        assert!(v["ndi"].get("machinename").is_none(), "the computer's name");
        let v = settings(&NdiSettings { machine: " REGIE-1 ".into(), ..NdiSettings::default() }, "");
        assert_eq!(v["ndi"]["machinename"], "REGIE-1");
    }

    #[test]
    fn four_cc_codes() {
        assert_eq!(FOURCC_UYVY, 0x5956_5955);
        assert_eq!(FOURCC_FLTP, 0x5054_4c46);
    }
}
