//! ST 2110-20 (vidéo non compressée) et ST 2110-30 (son PCM) par Intel Media Transport Library
//! (MTL), sur les ports média du serveur (Intel E810 : DPDK, AF_XDP, ou socket du noyau).
//!
//! - `libmtl.so` est chargée au lancement ([`load`]) : sans elle, le Core démarre quand même.
//! - Les images sont en v210 (comme les flux MXL) ; en entrelacé, MTL donne et prend des trames.
//! - Une instance MTL pour le serveur ([`Mtl::open`]), puis un récepteur ([`Rx`]) ou un émetteur
//!   ([`Tx`]) par flux, sur une ou deux pattes (SMPTE 2022-7 : rouge et bleue).
#[cfg(mtl_shim)]
use std::ffi::{c_char, c_int, c_void, CString};
use std::{
    net::Ipv4Addr,
    path::{Path, PathBuf},
};

/// Le pont n'a pas été construit avec le Core.
pub const BUILT: bool = cfg!(mtl_shim);

#[cfg(mtl_shim)]
mod ffi {
    use super::*;

    #[repr(C)]
    pub struct Port {
        pub name: [c_char; 64],
        pub pmd: c_int,
        pub ip: [u8; 4],
        pub mask: [u8; 4],
        pub gateway: [u8; 4],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Video {
        pub width: u32,
        pub height: u32,
        pub fps: c_int,
        pub interlaced: c_int,
    }

    #[repr(C)]
    pub struct Frame {
        pub frame: *mut c_void,
        pub data: *mut u8,
        pub linesize: u64,
        pub width: u32,
        pub height: u32,
        pub second_field: c_int,
        pub complete: c_int,
        pub timestamp: u64,
    }

    extern "C" {
        pub fn smtl_load(path: *const c_char, err: *mut c_char, errlen: c_int) -> c_int;
        pub fn smtl_open(ports: *const Port, n: c_int, tai_offset: u32, lcores: *const c_char, err: *mut c_char, errlen: c_int) -> *mut c_void;
        pub fn smtl_close(h: *mut c_void);
        pub fn smtl_port_stats(h: *mut c_void, port: c_int, rx_bytes: *mut u64, tx_bytes: *mut u64, rx_dropped: *mut u64) -> c_int;
        pub fn smtl_rx_open(
            h: *mut c_void,
            name: *const c_char,
            n: c_int,
            port_index: *const c_int,
            ip: *const [u8; 4],
            udp: *const u16,
            sip: *const [u8; 4],
            pt: u8,
            v: Video,
            err: *mut c_char,
            errlen: c_int,
        ) -> *mut c_void;
        pub fn smtl_rx_get(rx: *mut c_void, out: *mut Frame) -> c_int;
        pub fn smtl_rx_put(rx: *mut c_void, frame: *mut c_void);
        pub fn smtl_rx_stats(rx: *mut c_void, out: *mut u64);
        pub fn smtl_rx_close(rx: *mut c_void);
        pub fn smtl_tx_open(
            h: *mut c_void,
            name: *const c_char,
            n: c_int,
            port_index: *const c_int,
            ip: *const [u8; 4],
            udp: *const u16,
            pt: u8,
            v: Video,
            pacing: c_int,
            err: *mut c_char,
            errlen: c_int,
        ) -> *mut c_void;
        pub fn smtl_tx_get(tx: *mut c_void, out: *mut Frame) -> c_int;
        pub fn smtl_tx_put(tx: *mut c_void, frame: *mut c_void, second_field: c_int) -> c_int;
        pub fn smtl_tx_close(tx: *mut c_void);
    }

    pub fn text(buf: &[c_char]) -> String {
        let bytes: Vec<u8> = buf.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

#[cfg(not(mtl_shim))]
const NOT_BUILT: &str = "this Core was built without the MTL bridge";

/// Où chercher la bibliothèque : `STRATA_MTL_LIB`, celle livrée avec le Core, puis le système.
pub fn library_paths() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::env::var_os("STRATA_MTL_LIB").filter(|s| !s.is_empty()).map(PathBuf::from).into_iter().collect();
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.join("../lib/mtl/lib/libmtl.so"))) {
        v.push(dir);
    }
    // Livrée avec le Core (scripts/build-mtl.sh) : DPDK y cherche aussi ses pilotes.
    v.extend(["/opt/strata/lib/mtl/lib/libmtl.so", "/usr/local/lib/x86_64-linux-gnu/libmtl.so", "/usr/local/lib/libmtl.so", "libmtl.so"].map(PathBuf::from));
    v
}

/// Charge libmtl (la première trouvée) : rend son chemin.
pub fn load() -> Result<PathBuf, String> {
    let mut last = "libmtl.so not found".to_string();
    for p in library_paths() {
        match load_from(&p) {
            Ok(()) => return Ok(p),
            Err(e) => last = format!("{}: {e}", p.display()),
        }
    }
    Err(last)
}

pub fn load_from(path: &Path) -> Result<(), String> {
    #[cfg(mtl_shim)]
    {
        let c = CString::new(path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        let mut err = [0 as c_char; 256];
        // SAFETY: plain call; `err` sized as passed.
        if unsafe { ffi::smtl_load(c.as_ptr(), err.as_mut_ptr(), err.len() as c_int) } == 0 {
            Ok(())
        } else {
            Err(ffi::text(&err))
        }
    }
    #[cfg(not(mtl_shim))]
    {
        let _ = path;
        Err(NOT_BUILT.into())
    }
}

/// Comment MTL tient un port média.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pmd {
    /// DPDK : le port est pris au noyau (vfio-pci), le plus rapide. Nom : l'adresse PCI.
    Dpdk = 0,
    /// AF_XDP natif : le port reste au noyau. Nom : `native_af_xdp:<interface>`.
    AfXdp = 4,
    /// Socket du noyau : le plus simple, le moins rapide. Nom : `kernel:<interface>`.
    Kernel = 17,
}

/// Un port média.
#[derive(Debug, Clone)]
pub struct Port {
    pub name: String,
    pub pmd: Pmd,
    pub ip: Ipv4Addr,
    pub mask: Ipv4Addr,
    pub gateway: Ipv4Addr,
}

/// Une vidéo : taille de l'image entière, cadence en images par seconde, entrelacée ou non.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Video {
    pub width: u32,
    pub height: u32,
    pub rate: (u32, u32),
    pub interlaced: bool,
}

/// La cadence pour MTL (`enum st_fps`) : en entrelacé, celle des trames.
fn st_fps(v: &Video) -> Option<i32> {
    let r = v.rate.0 as f64 / v.rate.1.max(1) as f64 * if v.interlaced { 2.0 } else { 1.0 };
    let near = |x: f64| (r - x).abs() < 0.01;
    Some(if near(59.94) {
        0
    } else if near(50.0) {
        1
    } else if near(29.97) {
        2
    } else if near(25.0) {
        3
    } else if near(119.88) {
        4
    } else if near(120.0) {
        5
    } else if near(100.0) {
        6
    } else if near(60.0) {
        7
    } else if near(30.0) {
        8
    } else if near(24.0) {
        9
    } else if near(23.98) {
        10
    } else {
        return None;
    })
}

/// Une patte d'un flux : le port média (0 ou 1), l'adresse (multicast ou non) et le port UDP.
#[derive(Debug, Clone, Copy)]
pub struct Leg {
    pub port: usize,
    pub ip: Ipv4Addr,
    pub udp: u16,
    /// Réception : la seule source acceptée (multicast SSM, IGMPv3) ; None : toutes.
    pub source: Option<Ipv4Addr>,
}

/// Le cadencement d'un émetteur vidéo (ST 2110-21).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pacing {
    /// N : étroit (le plus courant).
    #[default]
    Narrow,
    /// NL : étroit linéaire.
    NarrowLinear,
    /// W : large.
    Wide,
}

/// Les compteurs d'une réception, depuis son ouverture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RxStats {
    /// Paquets acceptés (après 2022-7).
    pub received: u64,
    /// Paquets manquants sur l'une ou l'autre patte (avant 2022-7).
    pub lost: u64,
    /// Paquets perdus pour de bon (que l'autre patte n'a pas rattrapés).
    pub unrecovered: u64,
    /// Par patte (rouge, bleue) : paquets reçus, manquants.
    pub legs: [(u64, u64); 2],
}

impl RxStats {
    #[cfg(mtl_shim)]
    fn of(o: [u64; 7]) -> RxStats {
        RxStats { received: o[0], lost: o[1], unrecovered: o[2], legs: [(o[3], o[4]), (o[5], o[6])] }
    }
}

/// L'instance MTL du serveur.
pub struct Mtl {
    #[cfg(mtl_shim)]
    h: *mut c_void,
}

// SAFETY: MTL guards its instance; sessions are created and freed by the Core's own threads.
unsafe impl Send for Mtl {}
unsafe impl Sync for Mtl {}

impl Mtl {
    /// Ouvre MTL sur les ports média (1, ou 2 en 2022-7) ; l'heure TAI vient de l'horloge du
    /// système (tenue par PTP) plus `tai_offset` secondes.
    pub fn open(ports: &[Port], tai_offset: u32, lcores: Option<&str>) -> Result<Mtl, String> {
        #[cfg(mtl_shim)]
        {
            let raw: Vec<ffi::Port> = ports
                .iter()
                .map(|p| {
                    let mut name = [0 as c_char; 64];
                    for (d, s) in name.iter_mut().zip(p.name.bytes().take(63)) {
                        *d = s as c_char;
                    }
                    ffi::Port { name, pmd: p.pmd as c_int, ip: p.ip.octets(), mask: p.mask.octets(), gateway: p.gateway.octets() }
                })
                .collect();
            let lc = lcores.map(|l| CString::new(l).unwrap_or_default());
            let mut err = [0 as c_char; 256];
            // SAFETY: `raw` holds `n` ports; `err` sized as passed.
            let h = unsafe {
                ffi::smtl_open(
                    raw.as_ptr(),
                    raw.len() as c_int,
                    tai_offset,
                    lc.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
                    err.as_mut_ptr(),
                    err.len() as c_int,
                )
            };
            if h.is_null() {
                Err(ffi::text(&err))
            } else {
                Ok(Mtl { h })
            }
        }
        #[cfg(not(mtl_shim))]
        {
            let _ = (ports, tai_offset, lcores);
            Err(NOT_BUILT.into())
        }
    }

    /// Reçoit un flux vidéo.
    pub fn rx(&self, name: &str, legs: &[Leg], payload_type: u8, v: &Video) -> Result<Rx, String> {
        let fps = st_fps(v).ok_or("MTL has no such frame rate")?;
        #[cfg(mtl_shim)]
        {
            let (idx, ips, udps, sips) = legs_of(legs);
            let n = CString::new(name).unwrap_or_default();
            let mut err = [0 as c_char; 256];
            let vv = ffi::Video { width: v.width, height: v.height, fps, interlaced: v.interlaced as c_int };
            // SAFETY: arrays of `legs.len()` entries; `err` sized as passed.
            let h = unsafe {
                ffi::smtl_rx_open(
                    self.h,
                    n.as_ptr(),
                    legs.len() as c_int,
                    idx.as_ptr(),
                    ips.as_ptr(),
                    udps.as_ptr(),
                    sips.as_ptr(),
                    payload_type,
                    vv,
                    err.as_mut_ptr(),
                    err.len() as c_int,
                )
            };
            if h.is_null() {
                Err(ffi::text(&err))
            } else {
                Ok(Rx { h, _name: n })
            }
        }
        #[cfg(not(mtl_shim))]
        {
            let _ = (name, legs, payload_type, fps);
            Err(NOT_BUILT.into())
        }
    }

    /// Envoie un flux vidéo.
    pub fn tx(&self, name: &str, legs: &[Leg], payload_type: u8, v: &Video, pacing: Pacing) -> Result<Tx, String> {
        let fps = st_fps(v).ok_or("MTL has no such frame rate")?;
        #[cfg(mtl_shim)]
        {
            let (idx, ips, udps, _) = legs_of(legs);
            let n = CString::new(name).unwrap_or_default();
            let mut err = [0 as c_char; 256];
            let vv = ffi::Video { width: v.width, height: v.height, fps, interlaced: v.interlaced as c_int };
            // SAFETY: arrays of `legs.len()` entries; `err` sized as passed.
            let h = unsafe {
                ffi::smtl_tx_open(
                    self.h,
                    n.as_ptr(),
                    legs.len() as c_int,
                    idx.as_ptr(),
                    ips.as_ptr(),
                    udps.as_ptr(),
                    payload_type,
                    vv,
                    match pacing {
                        Pacing::Narrow => 0,
                        Pacing::Wide => 1,
                        Pacing::NarrowLinear => 2,
                    },
                    err.as_mut_ptr(),
                    err.len() as c_int,
                )
            };
            if h.is_null() {
                Err(ffi::text(&err))
            } else {
                Ok(Tx { h, _name: n })
            }
        }
        #[cfg(not(mtl_shim))]
        {
            let _ = (name, legs, payload_type, fps, pacing);
            Err(NOT_BUILT.into())
        }
    }
}

#[cfg(mtl_shim)]
#[allow(clippy::type_complexity)]
fn legs_of(legs: &[Leg]) -> (Vec<c_int>, Vec<[u8; 4]>, Vec<u16>, Vec<[u8; 4]>) {
    (
        legs.iter().map(|l| l.port as c_int).collect(),
        legs.iter().map(|l| l.ip.octets()).collect(),
        legs.iter().map(|l| l.udp).collect(),
        legs.iter().map(|l| l.source.map_or([0; 4], |a| a.octets())).collect(),
    )
}

/// Les compteurs d'un port média vus par MTL.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PortStats {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    /// Paquets jetés par la carte (files pleines).
    pub rx_dropped: u64,
}

impl Mtl {
    /// Les compteurs du port `port` (0 : port 1), si cette version de MTL les donne.
    pub fn port_stats(&self, port: usize) -> Option<PortStats> {
        #[cfg(mtl_shim)]
        {
            let mut p = PortStats::default();
            // SAFETY: `h` is open; the out-pointers are valid for the call.
            let r = unsafe { ffi::smtl_port_stats(self.h, port as c_int, &mut p.rx_bytes, &mut p.tx_bytes, &mut p.rx_dropped) };
            (r == 0).then_some(p)
        }
        #[cfg(not(mtl_shim))]
        {
            let _ = port;
            None
        }
    }
}

impl Drop for Mtl {
    fn drop(&mut self) {
        #[cfg(mtl_shim)]
        // SAFETY: opened by `open`, closed once; its sessions are dropped before (they borrow nothing
        // but the Core stops them first).
        unsafe {
            ffi::smtl_close(self.h)
        };
    }
}

/// Une image prise à MTL (une trame en entrelacé), rendue en la lâchant.
pub struct Frame<'a> {
    pub data: &'a mut [u8],
    pub linesize: usize,
    pub width: u32,
    pub height: u32,
    pub second_field: bool,
    pub complete: bool,
    #[cfg(mtl_shim)]
    raw: *mut c_void,
    #[cfg(mtl_shim)]
    owner: *mut c_void,
    tx: bool,
    sent: bool,
}

impl Frame<'_> {
    /// Envoie l'image remplie (émetteur) ; en entrelacé, `second` : la trame du bas.
    pub fn send_field(mut self, second: bool) {
        self.sent = true;
        #[cfg(mtl_shim)]
        // SAFETY: a frame taken from this sender, given back once.
        unsafe {
            ffi::smtl_tx_put(self.owner, self.raw, second as c_int);
        }
        #[cfg(not(mtl_shim))]
        let _ = second;
    }
}

impl Drop for Frame<'_> {
    fn drop(&mut self) {
        if self.sent {
            return;
        }
        #[cfg(mtl_shim)]
        // SAFETY: a frame taken from this session, given back once.
        unsafe {
            if self.tx {
                ffi::smtl_tx_put(self.owner, self.raw, self.second_field as c_int);
            } else {
                ffi::smtl_rx_put(self.owner, self.raw);
            }
        }
        let _ = self.tx;
    }
}

#[cfg(mtl_shim)]
fn frame_of<'a>(f: ffi::Frame, owner: *mut c_void, tx: bool) -> Frame<'a> {
    let len = f.linesize as usize * f.height as usize;
    Frame {
        // SAFETY: MTL hands a buffer of `height` rows of `linesize` bytes, ours until put back.
        data: unsafe { std::slice::from_raw_parts_mut(f.data, len) },
        linesize: f.linesize as usize,
        width: f.width,
        height: f.height,
        second_field: f.second_field != 0,
        complete: f.complete != 0,
        raw: f.frame,
        owner,
        tx,
        sent: false,
    }
}

/// Un flux reçu.
pub struct Rx {
    #[cfg(mtl_shim)]
    h: *mut c_void,
    #[cfg(mtl_shim)]
    _name: CString,
}

unsafe impl Send for Rx {}

impl Rx {
    /// La prochaine image (200 ms au plus).
    pub fn next(&self) -> Option<Frame<'_>> {
        #[cfg(mtl_shim)]
        {
            let mut f = ffi::Frame {
                frame: std::ptr::null_mut(),
                data: std::ptr::null_mut(),
                linesize: 0,
                width: 0,
                height: 0,
                second_field: 0,
                complete: 0,
                timestamp: 0,
            };
            // SAFETY: open session; `f` is a valid out pointer.
            (unsafe { ffi::smtl_rx_get(self.h, &mut f) } == 1).then(|| frame_of(f, self.h, false))
        }
        #[cfg(not(mtl_shim))]
        None
    }

    /// Les compteurs de la réception (par patte aussi).
    pub fn stats(&self) -> RxStats {
        #[cfg(mtl_shim)]
        {
            let mut o = [0u64; 7];
            // SAFETY: open session; `o` holds the 7 counters the shim writes.
            unsafe { ffi::smtl_rx_stats(self.h, o.as_mut_ptr()) };
            RxStats::of(o)
        }
        #[cfg(not(mtl_shim))]
        RxStats::default()
    }
}

impl Drop for Rx {
    fn drop(&mut self) {
        #[cfg(mtl_shim)]
        // SAFETY: opened by `rx`, freed once.
        unsafe {
            ffi::smtl_rx_close(self.h)
        };
    }
}

/// Un flux envoyé.
pub struct Tx {
    #[cfg(mtl_shim)]
    h: *mut c_void,
    #[cfg(mtl_shim)]
    _name: CString,
}

unsafe impl Send for Tx {}

impl Tx {
    /// Une image à remplir (200 ms au plus), puis [`Frame::send`].
    pub fn next(&self) -> Option<Frame<'_>> {
        #[cfg(mtl_shim)]
        {
            let mut f = ffi::Frame {
                frame: std::ptr::null_mut(),
                data: std::ptr::null_mut(),
                linesize: 0,
                width: 0,
                height: 0,
                second_field: 0,
                complete: 0,
                timestamp: 0,
            };
            // SAFETY: open session; `f` is a valid out pointer.
            (unsafe { ffi::smtl_tx_get(self.h, &mut f) } == 1).then(|| frame_of(f, self.h, true))
        }
        #[cfg(not(mtl_shim))]
        None
    }
}

impl Drop for Tx {
    fn drop(&mut self) {
        #[cfg(mtl_shim)]
        // SAFETY: opened by `tx`, freed once.
        unsafe {
            ffi::smtl_tx_close(self.h)
        };
    }
}

// ---------- Le son : ST 2110-30 ----------

/// Un flux audio PCM à 48 kHz : canaux, échantillons de 16 ou 24 bits, paquets de 1 ms (ou
/// 125 µs). Le Core échange avec MTL des blocs de `frame_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Audio {
    pub channels: u16,
    /// 16 (L16) ou 24 (L24).
    pub bits: u8,
    /// La durée d'un paquet, en µs : 1000 (le cas courant, AES67) ou 125.
    pub ptime_us: u32,
    pub frame_ms: u32,
}

impl Audio {
    /// Les octets d'un échantillon d'un canal.
    pub fn sample_bytes(&self) -> usize {
        self.bits as usize / 8
    }

    fn fmt(&self) -> Option<c_int> {
        match self.bits {
            16 => Some(1),
            24 => Some(2),
            _ => None,
        }
    }

    fn ptime(&self) -> Option<c_int> {
        match self.ptime_us {
            1000 => Some(0),
            125 => Some(1),
            250 => Some(2),
            4000 => Some(4),
            _ => None,
        }
    }
}

#[cfg(mtl_shim)]
mod affi {
    use super::*;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Audio {
        pub channels: u16,
        pub fmt: c_int,
        pub ptime: c_int,
        pub frame_ms: u32,
    }

    #[repr(C)]
    pub struct AFrame {
        pub frame: *mut c_void,
        pub data: *mut u8,
        pub size: u64,
        pub timestamp: u64,
        pub complete: c_int,
    }

    extern "C" {
        pub fn smtl_arx_open(
            h: *mut c_void,
            name: *const c_char,
            n: c_int,
            port_index: *const c_int,
            ip: *const [u8; 4],
            udp: *const u16,
            sip: *const [u8; 4],
            pt: u8,
            a: Audio,
            err: *mut c_char,
            errlen: c_int,
        ) -> *mut c_void;
        pub fn smtl_arx_get(rx: *mut c_void, out: *mut AFrame) -> c_int;
        pub fn smtl_arx_put(rx: *mut c_void, frame: *mut c_void);
        pub fn smtl_arx_stats(rx: *mut c_void, out: *mut u64);
        pub fn smtl_arx_close(rx: *mut c_void);
        pub fn smtl_atx_open(
            h: *mut c_void,
            name: *const c_char,
            n: c_int,
            port_index: *const c_int,
            ip: *const [u8; 4],
            udp: *const u16,
            sip: *const [u8; 4],
            pt: u8,
            a: Audio,
            err: *mut c_char,
            errlen: c_int,
        ) -> *mut c_void;
        pub fn smtl_atx_get(tx: *mut c_void, out: *mut AFrame) -> c_int;
        pub fn smtl_atx_put(tx: *mut c_void, frame: *mut c_void) -> c_int;
        pub fn smtl_atx_close(tx: *mut c_void);
    }

    pub fn empty() -> AFrame {
        AFrame { frame: std::ptr::null_mut(), data: std::ptr::null_mut(), size: 0, timestamp: 0, complete: 0 }
    }
}

impl Mtl {
    #[cfg(mtl_shim)]
    fn audio_open(&self, tx: bool, name: &str, legs: &[Leg], payload_type: u8, a: &Audio) -> Result<(*mut c_void, CString), String> {
        let fmt = a.fmt().ok_or("ST 2110-30: 16 or 24-bit samples")?;
        let ptime = a.ptime().ok_or("ST 2110-30: 1 ms or 125 µs packets")?;
        let (idx, ips, udps, sips) = legs_of(legs);
        let n = CString::new(name).unwrap_or_default();
        let mut err = [0 as c_char; 256];
        let aa = affi::Audio { channels: a.channels, fmt, ptime, frame_ms: a.frame_ms.max(1) };
        let open = if tx { affi::smtl_atx_open } else { affi::smtl_arx_open };
        // SAFETY: arrays of `legs.len()` entries; `err` sized as passed.
        let h = unsafe {
            open(
                self.h,
                n.as_ptr(),
                legs.len() as c_int,
                idx.as_ptr(),
                ips.as_ptr(),
                udps.as_ptr(),
                sips.as_ptr(),
                payload_type,
                aa,
                err.as_mut_ptr(),
                err.len() as c_int,
            )
        };
        if h.is_null() {
            Err(ffi::text(&err))
        } else {
            Ok((h, n))
        }
    }

    /// Reçoit un flux audio.
    pub fn arx(&self, name: &str, legs: &[Leg], payload_type: u8, a: &Audio) -> Result<ARx, String> {
        #[cfg(mtl_shim)]
        {
            self.audio_open(false, name, legs, payload_type, a).map(|(h, n)| ARx { h, _name: n, audio: *a })
        }
        #[cfg(not(mtl_shim))]
        {
            let _ = (name, legs, payload_type, a);
            Err(NOT_BUILT.into())
        }
    }

    /// Envoie un flux audio.
    pub fn atx(&self, name: &str, legs: &[Leg], payload_type: u8, a: &Audio) -> Result<ATx, String> {
        #[cfg(mtl_shim)]
        {
            self.audio_open(true, name, legs, payload_type, a).map(|(h, n)| ATx { h, _name: n, audio: *a })
        }
        #[cfg(not(mtl_shim))]
        {
            let _ = (name, legs, payload_type, a);
            Err(NOT_BUILT.into())
        }
    }
}

/// Un flux audio reçu.
pub struct ARx {
    #[cfg(mtl_shim)]
    h: *mut c_void,
    #[cfg(mtl_shim)]
    _name: CString,
    pub audio: Audio,
}

unsafe impl Send for ARx {}

impl ARx {
    /// Le prochain bloc (200 ms au plus), en float entrelacés.
    pub fn next(&self) -> Option<Vec<f32>> {
        #[cfg(mtl_shim)]
        {
            let mut f = affi::empty();
            // SAFETY: open session; `f` is a valid out pointer.
            if unsafe { affi::smtl_arx_get(self.h, &mut f) } != 1 {
                return None;
            }
            // SAFETY: MTL hands `size` bytes, ours until put back just below.
            let data = unsafe { std::slice::from_raw_parts(f.data, f.size as usize) };
            let pcm = from_pcm(data, self.audio.bits);
            // SAFETY: a frame taken from this session, given back once.
            unsafe { affi::smtl_arx_put(self.h, f.frame) };
            Some(pcm)
        }
        #[cfg(not(mtl_shim))]
        None
    }

    /// Les compteurs de la réception (par patte aussi).
    pub fn stats(&self) -> RxStats {
        #[cfg(mtl_shim)]
        {
            let mut o = [0u64; 7];
            // SAFETY: open session; `o` holds the 7 counters the shim writes.
            unsafe { affi::smtl_arx_stats(self.h, o.as_mut_ptr()) };
            RxStats::of(o)
        }
        #[cfg(not(mtl_shim))]
        RxStats::default()
    }
}

impl Drop for ARx {
    fn drop(&mut self) {
        #[cfg(mtl_shim)]
        // SAFETY: opened by `arx`, freed once.
        unsafe {
            affi::smtl_arx_close(self.h)
        };
    }
}

/// Un flux audio envoyé.
pub struct ATx {
    #[cfg(mtl_shim)]
    h: *mut c_void,
    #[cfg(mtl_shim)]
    _name: CString,
    pub audio: Audio,
}

unsafe impl Send for ATx {}

impl ATx {
    /// Les échantillons (par canal) d'un bloc à envoyer.
    pub fn frame_samples(&self) -> usize {
        48 * self.audio.frame_ms as usize
    }

    /// Attend un bloc libre (200 ms au plus) et l'envoie, rempli de `pcm` (float entrelacés,
    /// complété de silence) : vrai s'il est parti. MTL cadence l'envoi : appelé en boucle, il
    /// donne le rythme.
    pub fn send(&self, pcm: &[f32]) -> bool {
        #[cfg(mtl_shim)]
        {
            let mut f = affi::empty();
            // SAFETY: open session; `f` is a valid out pointer.
            if unsafe { affi::smtl_atx_get(self.h, &mut f) } != 1 {
                return false;
            }
            // SAFETY: MTL hands `size` bytes, ours until put back just below.
            let data = unsafe { std::slice::from_raw_parts_mut(f.data, f.size as usize) };
            to_pcm(pcm, data, self.audio.bits);
            // SAFETY: a frame taken from this session, given back once.
            unsafe { affi::smtl_atx_put(self.h, f.frame) == 0 }
        }
        #[cfg(not(mtl_shim))]
        {
            let _ = pcm;
            false
        }
    }
}

impl Drop for ATx {
    fn drop(&mut self) {
        #[cfg(mtl_shim)]
        // SAFETY: opened by `atx`, freed once.
        unsafe {
            affi::smtl_atx_close(self.h)
        };
    }
}

// ---------- Les données annexes : ST 2110-40 ----------

/// Un paquet ANC (SMPTE ST 291) : les mots sur 8 bits (MTL calcule parité et somme de contrôle).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Anc {
    pub c: bool,
    pub line: u16,
    pub offset: u16,
    pub s: bool,
    pub stream: u8,
    pub did: u8,
    pub sdid: u8,
    pub udw: Vec<u8>,
}

/// Les paquets d'un bloc (20 au plus).
pub const ANC_MAX: usize = 20;

#[cfg(mtl_shim)]
mod dffi {
    use super::*;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Anc {
        pub c: u16,
        pub line: u16,
        pub offset: u16,
        pub s: u16,
        pub stream: u16,
        pub did: u16,
        pub sdid: u16,
        pub size: u16,
        pub udw: [u8; 255],
    }

    pub const EMPTY: Anc = Anc { c: 0, line: 0, offset: 0, s: 0, stream: 0, did: 0, sdid: 0, size: 0, udw: [0; 255] };

    extern "C" {
        pub fn smtl_drx_open(
            h: *mut c_void,
            name: *const c_char,
            n: c_int,
            port_index: *const c_int,
            ip: *const [u8; 4],
            udp: *const u16,
            sip: *const [u8; 4],
            pt: u8,
            interlaced: c_int,
            err: *mut c_char,
            errlen: c_int,
        ) -> *mut c_void;
        pub fn smtl_drx_get(rx: *mut c_void, out: *mut Anc, max: c_int, field: *mut c_int) -> c_int;
        pub fn smtl_drx_stats(rx: *mut c_void, out: *mut u64);
        pub fn smtl_drx_close(rx: *mut c_void);
        pub fn smtl_dtx_open(
            h: *mut c_void,
            name: *const c_char,
            n: c_int,
            port_index: *const c_int,
            ip: *const [u8; 4],
            udp: *const u16,
            pt: u8,
            fps: c_int,
            interlaced: c_int,
            err: *mut c_char,
            errlen: c_int,
        ) -> *mut c_void;
        pub fn smtl_dtx_send(tx: *mut c_void, packets: *const Anc, n: c_int) -> c_int;
        pub fn smtl_dtx_close(tx: *mut c_void);
    }
}

impl Mtl {
    /// Reçoit un flux de données.
    pub fn drx(&self, name: &str, legs: &[Leg], payload_type: u8, interlaced: bool) -> Result<DRx, String> {
        #[cfg(mtl_shim)]
        {
            let (idx, ips, udps, sips) = legs_of(legs);
            let n = CString::new(name).unwrap_or_default();
            let mut err = [0 as c_char; 256];
            // SAFETY: arrays of `legs.len()` entries; `err` sized as passed.
            let h = unsafe {
                dffi::smtl_drx_open(
                    self.h,
                    n.as_ptr(),
                    legs.len() as c_int,
                    idx.as_ptr(),
                    ips.as_ptr(),
                    udps.as_ptr(),
                    sips.as_ptr(),
                    payload_type,
                    interlaced as c_int,
                    err.as_mut_ptr(),
                    err.len() as c_int,
                )
            };
            if h.is_null() {
                return Err(ffi::text(&err));
            }
            Ok(DRx { h, _name: n, buf: std::sync::Mutex::new(vec![dffi::EMPTY; ANC_MAX]) })
        }
        #[cfg(not(mtl_shim))]
        {
            let _ = (name, legs, payload_type, interlaced);
            Err(NOT_BUILT.into())
        }
    }

    /// Envoie un flux de données, au rythme de la vidéo `v` (une salve par trame en entrelacé).
    pub fn dtx(&self, name: &str, legs: &[Leg], payload_type: u8, v: &Video) -> Result<DTx, String> {
        #[cfg(mtl_shim)]
        {
            let fps = st_fps(v).ok_or("ST 2110-40: unsupported frame rate")?;
            let (idx, ips, udps, _) = legs_of(legs);
            let n = CString::new(name).unwrap_or_default();
            let mut err = [0 as c_char; 256];
            // SAFETY: arrays of `legs.len()` entries; `err` sized as passed.
            let h = unsafe {
                dffi::smtl_dtx_open(
                    self.h,
                    n.as_ptr(),
                    legs.len() as c_int,
                    idx.as_ptr(),
                    ips.as_ptr(),
                    udps.as_ptr(),
                    payload_type,
                    fps,
                    v.interlaced as c_int,
                    err.as_mut_ptr(),
                    err.len() as c_int,
                )
            };
            if h.is_null() {
                return Err(ffi::text(&err));
            }
            Ok(DTx { h, _name: n, buf: std::sync::Mutex::new(Vec::with_capacity(ANC_MAX)) })
        }
        #[cfg(not(mtl_shim))]
        {
            let _ = (name, legs, payload_type, v);
            Err(NOT_BUILT.into())
        }
    }
}

/// Un flux de données reçu.
pub struct DRx {
    #[cfg(mtl_shim)]
    h: *mut c_void,
    #[cfg(mtl_shim)]
    _name: CString,
    #[cfg(mtl_shim)]
    buf: std::sync::Mutex<Vec<dffi::Anc>>,
}

unsafe impl Send for DRx {}
unsafe impl Sync for DRx {}

impl DRx {
    /// Le prochain bloc (200 ms au plus) : ses paquets et son champ F (0 progressif, 2 première
    /// trame, 3 seconde).
    pub fn next(&self) -> Option<(Vec<Anc>, u8)> {
        #[cfg(mtl_shim)]
        {
            let mut buf = self.buf.lock().unwrap_or_else(|e| e.into_inner());
            let mut field: c_int = 0;
            // SAFETY: open session; `buf` holds ANC_MAX entries.
            let n = unsafe { dffi::smtl_drx_get(self.h, buf.as_mut_ptr(), ANC_MAX as c_int, &mut field) };
            if n < 0 {
                return None;
            }
            let out = buf[..n as usize]
                .iter()
                .map(|a| Anc {
                    c: a.c != 0,
                    line: a.line,
                    offset: a.offset,
                    s: a.s != 0,
                    stream: a.stream as u8,
                    did: a.did as u8,
                    sdid: a.sdid as u8,
                    udw: a.udw[..(a.size as usize).min(255)].to_vec(),
                })
                .collect();
            Some((out, field as u8))
        }
        #[cfg(not(mtl_shim))]
        None
    }

    /// Les compteurs de la réception (par patte aussi).
    pub fn stats(&self) -> RxStats {
        #[cfg(mtl_shim)]
        {
            let mut o = [0u64; 7];
            // SAFETY: open session; `o` holds the 7 counters the shim writes.
            unsafe { dffi::smtl_drx_stats(self.h, o.as_mut_ptr()) };
            RxStats::of(o)
        }
        #[cfg(not(mtl_shim))]
        RxStats::default()
    }
}

impl Drop for DRx {
    fn drop(&mut self) {
        #[cfg(mtl_shim)]
        // SAFETY: opened by `drx`, freed once.
        unsafe {
            dffi::smtl_drx_close(self.h)
        };
    }
}

/// Un flux de données envoyé.
pub struct DTx {
    #[cfg(mtl_shim)]
    h: *mut c_void,
    #[cfg(mtl_shim)]
    _name: CString,
    #[cfg(mtl_shim)]
    buf: std::sync::Mutex<Vec<dffi::Anc>>,
}

unsafe impl Send for DTx {}
unsafe impl Sync for DTx {}

impl DTx {
    /// Attend un bloc libre (200 ms au plus) et l'envoie avec ces paquets (20 au plus ; aucun :
    /// un bloc vide). Vrai s'il est parti. MTL cadence l'envoi : appelé en boucle, il donne le
    /// rythme (une fois par image, par trame en entrelacé).
    pub fn send(&self, packets: &[Anc]) -> bool {
        #[cfg(mtl_shim)]
        {
            let mut buf = self.buf.lock().unwrap_or_else(|e| e.into_inner());
            buf.clear();
            for p in packets.iter().take(ANC_MAX) {
                let mut a = dffi::EMPTY;
                a.c = p.c as u16;
                a.line = p.line;
                a.offset = p.offset;
                a.s = p.s as u16;
                a.stream = p.stream as u16;
                a.did = p.did as u16;
                a.sdid = p.sdid as u16;
                let n = p.udw.len().min(255);
                a.size = n as u16;
                a.udw[..n].copy_from_slice(&p.udw[..n]);
                buf.push(a);
            }
            // SAFETY: open session; `buf` holds `buf.len()` entries.
            unsafe { dffi::smtl_dtx_send(self.h, buf.as_ptr(), buf.len() as c_int) == 0 }
        }
        #[cfg(not(mtl_shim))]
        {
            let _ = packets;
            false
        }
    }
}

impl Drop for DTx {
    fn drop(&mut self) {
        #[cfg(mtl_shim)]
        // SAFETY: opened by `dtx`, freed once.
        unsafe {
            dffi::smtl_dtx_close(self.h)
        };
    }
}

/// PCM du réseau (gros-boutien, signé, entrelacé) → float.
pub fn from_pcm(data: &[u8], bits: u8) -> Vec<f32> {
    match bits {
        16 => data.as_chunks::<2>().0.iter().map(|b| i16::from_be_bytes(*b) as f32 / 32768.0).collect(),
        _ => data.as_chunks::<3>().0.iter().map(|b| (i32::from_be_bytes([b[0], b[1], b[2], 0]) >> 8) as f32 / 8_388_608.0).collect(),
    }
}

/// float → PCM du réseau ; `out` complété de silence.
pub fn to_pcm(pcm: &[f32], out: &mut [u8], bits: u8) {
    out.fill(0);
    match bits {
        16 => {
            for (d, x) in out.as_chunks_mut::<2>().0.iter_mut().zip(pcm) {
                *d = ((x.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_be_bytes();
            }
        }
        _ => {
            for (d, x) in out.as_chunks_mut::<3>().0.iter_mut().zip(pcm) {
                let v = (x.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32;
                let b = v.to_be_bytes();
                *d = [b[1], b[2], b[3]];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_rates_for_mtl() {
        let v = |w, h, n, d, i| Video { width: w, height: h, rate: (n, d), interlaced: i };
        assert_eq!(st_fps(&v(1920, 1080, 25, 1, true)), Some(1));
        assert_eq!(st_fps(&v(1920, 1080, 50, 1, false)), Some(1));
        assert_eq!(st_fps(&v(1920, 1080, 30000, 1001, true)), Some(0));
        assert_eq!(st_fps(&v(3840, 2160, 25, 1, false)), Some(3));
        assert_eq!(st_fps(&v(1920, 1080, 48, 1, false)), None);
    }

    #[test]
    fn pcm_both_ways() {
        let x = [0.5f32, -0.5, 0.0, 0.999, -1.0];
        for bits in [16u8, 24] {
            let mut b = vec![0u8; x.len() * bits as usize / 8 + 3];
            to_pcm(&x, &mut b, bits);
            let y = from_pcm(&b, bits);
            for (a, c) in x.iter().zip(&y) {
                assert!((a - c).abs() < 1e-4, "{bits} bits: {a} → {c}");
            }
            // Le reste : du silence.
            assert_eq!(*y.last().unwrap(), 0.0);
        }
        // L24 : 0x400000 = 0,5 ; gros-boutien.
        assert_eq!(from_pcm(&[0x40, 0, 0, 0xC0, 0, 0], 24), vec![0.5, -0.5]);
    }
}
