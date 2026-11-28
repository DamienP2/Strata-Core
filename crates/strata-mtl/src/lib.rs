//! ST 2110-20 (vidéo non compressée) et ST 2110-30 (son PCM) par Intel Media Transport Library
//! (MTL), sur les ports média du serveur (Intel E810 : DPDK, AF_XDP, ou socket du noyau).
//!
//! - `libmtl.so` est chargée au lancement ([`load`]) : sans elle, le Core démarre quand même.
//! - Les images sont en v210 (comme les flux MXL) ; en entrelacé, MTL donne et prend des trames.
//! - ST 2110-22 (JPEG XS, par le greffon SVT-JPEG-XS de MTL) : [`Mtl::rx22`] et [`Mtl::tx22`]
//!   donnent les mêmes images v210 ; la conversion vers les plans du greffon se fait ici.
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
            ssrc: u32,
            user_time: c_int,
            err: *mut c_char,
            errlen: c_int,
        ) -> *mut c_void;
        pub fn smtl_tx_get(tx: *mut c_void, out: *mut Frame) -> c_int;
        pub fn smtl_tx_put(tx: *mut c_void, frame: *mut c_void, second_field: c_int, tai_ns: u64) -> c_int;
        pub fn smtl_tx_close(tx: *mut c_void);
        pub fn smtl_has_st22p() -> c_int;
        pub fn smtl22_rx_open(
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
        pub fn smtl22_rx_get(rx: *mut c_void, out: *mut Frame3) -> c_int;
        pub fn smtl22_rx_put(rx: *mut c_void, frame: *mut c_void);
        pub fn smtl22_rx_close(rx: *mut c_void);
        pub fn smtl22_tx_open(
            h: *mut c_void,
            name: *const c_char,
            n: c_int,
            port_index: *const c_int,
            ip: *const [u8; 4],
            udp: *const u16,
            pt: u8,
            v: Video,
            codestream_size: u64,
            ssrc: u32,
            user_time: c_int,
            err: *mut c_char,
            errlen: c_int,
        ) -> *mut c_void;
        pub fn smtl22_tx_get(tx: *mut c_void, out: *mut Frame3) -> c_int;
        pub fn smtl22_tx_put(tx: *mut c_void, frame: *mut c_void, second_field: c_int, tai_ns: u64) -> c_int;
        pub fn smtl22_tx_close(tx: *mut c_void);
    }

    /// Une image en trois plans (Y, Cb, Cr ; 10 bits dans 16).
    #[repr(C)]
    pub struct Frame3 {
        pub frame: *mut c_void,
        pub data: [*mut u8; 3],
        pub linesize: [u64; 3],
        pub width: u32,
        pub height: u32,
        pub second_field: c_int,
        pub complete: c_int,
        pub timestamp: u64,
    }

    impl Frame3 {
        pub fn empty() -> Frame3 {
            Frame3 {
                frame: std::ptr::null_mut(),
                data: [std::ptr::null_mut(); 3],
                linesize: [0; 3],
                width: 0,
                height: 0,
                second_field: 0,
                complete: 0,
                timestamp: 0,
            }
        }
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
                Ok(Rx { h, _name: n, native: !v.width.is_multiple_of(6), staging: Vec::new() })
            }
        }
        #[cfg(not(mtl_shim))]
        {
            let _ = (name, legs, payload_type, fps);
            Err(NOT_BUILT.into())
        }
    }

    /// Envoie un flux vidéo. `timing` (IPMX) : son SSRC, et chaque image part à l'instant de
    /// son `timestamp` (son horodatage RTP).
    pub fn tx(&self, name: &str, legs: &[Leg], payload_type: u8, v: &Video, pacing: Pacing, timing: Option<u32>) -> Result<Tx, String> {
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
                    timing.unwrap_or(0),
                    timing.is_some() as c_int,
                    err.as_mut_ptr(),
                    err.len() as c_int,
                )
            };
            if h.is_null() {
                Err(ffi::text(&err))
            } else {
                Ok(Tx { h, _name: n, native: !v.width.is_multiple_of(6), staging: Vec::new() })
            }
        }
        #[cfg(not(mtl_shim))]
        {
            let _ = (name, legs, payload_type, fps, pacing, timing);
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
    /// Émetteur : l'instant TAI (ns) où l'envoyer, son horodatage RTP (IPMX) ; 0 : le rythme de
    /// MTL.
    pub timestamp: u64,
    #[cfg(mtl_shim)]
    raw: *mut c_void,
    #[cfg(mtl_shim)]
    owner: *mut c_void,
    kind: Kind,
    sent: bool,
}

/// D'où vient une image : rendue à la bonne session ; une image JPEG XS à envoyer est d'abord
/// convertie de v210 vers les plans du greffon.
enum Kind {
    Rx,
    Tx,
    #[cfg_attr(not(mtl_shim), allow(dead_code))]
    Rx22,
    #[cfg_attr(not(mtl_shim), allow(dead_code))]
    Tx22(Planes),
    #[cfg_attr(not(mtl_shim), allow(dead_code))]
    TxNative(Native),
}

/// Une image MTL au format du réseau (RFC 4175), remplie depuis le v210 à l'envoi.
#[cfg_attr(not(mtl_shim), allow(dead_code))]
struct Native {
    ptr: *mut u8,
    linesize: usize,
    width: usize,
    lines: usize,
}

/// Les trois plans d'une image du greffon.
#[cfg_attr(not(mtl_shim), allow(dead_code))]
struct Planes {
    ptr: [*mut u8; 3],
    linesize: [usize; 3],
    width: usize,
    lines: usize,
}

impl Planes {
    /// SAFETY: the planes are an MTL frame of `lines` rows of `linesize` bytes each, ours until
    /// put back.
    #[cfg_attr(not(mtl_shim), allow(dead_code))]
    unsafe fn slices<'a>(&self) -> [&'a mut [u8]; 3] {
        let l = |i: usize| std::slice::from_raw_parts_mut(self.ptr[i], self.linesize[i] * self.lines);
        [l(0), l(1), l(2)]
    }
}

impl Frame<'_> {
    /// Envoie l'image remplie (émetteur) ; en entrelacé, `second` : la trame du bas.
    pub fn send_field(mut self, second: bool) {
        self.sent = true;
        self.put(second);
    }

    /// Rend l'image à sa session (envoyée, pour un émetteur).
    fn put(&mut self, second: bool) {
        #[cfg(mtl_shim)]
        // SAFETY: a frame taken from this session, given back once.
        unsafe {
            match &self.kind {
                Kind::Rx => ffi::smtl_rx_put(self.owner, self.raw),
                Kind::Tx => {
                    ffi::smtl_tx_put(self.owner, self.raw, second as c_int, self.timestamp);
                }
                Kind::Rx22 => ffi::smtl22_rx_put(self.owner, self.raw),
                Kind::TxNative(n) => {
                    let dst = std::slice::from_raw_parts_mut(n.ptr, n.linesize * n.lines);
                    for (s, d) in self.data.chunks_exact(self.linesize).zip(dst.chunks_exact_mut(n.linesize)) {
                        v210_to_rfc4175(s, d, n.width);
                    }
                    ffi::smtl_tx_put(self.owner, self.raw, second as c_int, self.timestamp);
                }
                Kind::Tx22(p) => {
                    let [y, cb, cr] = p.slices();
                    v210_to_planar(self.data, self.linesize, p.width, p.lines, [y, cb, cr], p.linesize);
                    ffi::smtl22_tx_put(self.owner, self.raw, second as c_int, self.timestamp);
                }
            }
        }
        #[cfg(not(mtl_shim))]
        let _ = (second, &self.kind);
    }
}

impl Drop for Frame<'_> {
    fn drop(&mut self) {
        if self.sent {
            return;
        }
        self.put(self.second_field);
    }
}

#[cfg(mtl_shim)]
fn frame_of<'a>(f: ffi::Frame, owner: *mut c_void, tx: bool) -> Frame<'a> {
    let kind = if tx { Kind::Tx } else { Kind::Rx };
    let len = f.linesize as usize * f.height as usize;
    Frame {
        // SAFETY: MTL hands a buffer of `height` rows of `linesize` bytes, ours until put back.
        data: unsafe { std::slice::from_raw_parts_mut(f.data, len) },
        linesize: f.linesize as usize,
        width: f.width,
        height: f.height,
        second_field: f.second_field != 0,
        complete: f.complete != 0,
        // Reçue : son horodatage RTP.
        timestamp: if tx { 0 } else { f.timestamp },
        raw: f.frame,
        owner,
        kind,
        sent: false,
    }
}

/// Un flux reçu.
pub struct Rx {
    #[cfg(mtl_shim)]
    h: *mut c_void,
    #[cfg(mtl_shim)]
    _name: CString,
    /// MTL rend le format du réseau (RFC 4175 : largeur non multiple de 6), converti ici en v210.
    #[cfg(mtl_shim)]
    native: bool,
    #[cfg(mtl_shim)]
    staging: Vec<u8>,
}

unsafe impl Send for Rx {}

impl Rx {
    /// La prochaine image (200 ms au plus), en v210 ; `timestamp` : son horodatage RTP (90 kHz).
    pub fn frame(&mut self) -> Option<Frame<'_>> {
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
            if unsafe { ffi::smtl_rx_get(self.h, &mut f) } != 1 {
                return None;
            }
            if !self.native {
                return Some(frame_of(f, self.h, false));
            }
            let (w, lines, ls) = (f.width as usize, f.height as usize, f.linesize as usize);
            let stride = v210_stride(f.width);
            self.staging.resize(stride * lines, 0);
            // SAFETY: MTL hands `lines` rows of `linesize` bytes, ours until put back.
            let src = unsafe { std::slice::from_raw_parts(f.data, ls * lines) };
            for (s, d) in src.chunks_exact(ls).zip(self.staging.chunks_exact_mut(stride)) {
                rfc4175_to_v210(s, d, w);
            }
            Some(Frame {
                data: &mut self.staging[..],
                linesize: stride,
                width: f.width,
                height: f.height,
                second_field: f.second_field != 0,
                complete: f.complete != 0,
                timestamp: f.timestamp,
                raw: f.frame,
                owner: self.h,
                kind: Kind::Rx,
                sent: false,
            })
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
    /// MTL attend le format du réseau (RFC 4175 : largeur non multiple de 6) : l'image remplie en
    /// v210 est convertie à l'envoi.
    #[cfg(mtl_shim)]
    native: bool,
    #[cfg(mtl_shim)]
    staging: Vec<u8>,
}

unsafe impl Send for Tx {}

impl Tx {
    /// Une image à remplir en v210 (200 ms au plus), puis [`Frame::send_field`].
    pub fn frame(&mut self) -> Option<Frame<'_>> {
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
            if unsafe { ffi::smtl_tx_get(self.h, &mut f) } != 1 {
                return None;
            }
            if !self.native {
                return Some(frame_of(f, self.h, true));
            }
            let lines = f.height as usize;
            let stride = v210_stride(f.width);
            self.staging.resize(stride * lines, 0);
            Some(Frame {
                data: &mut self.staging[..],
                linesize: stride,
                width: f.width,
                height: f.height,
                second_field: false,
                complete: true,
                timestamp: 0,
                raw: f.frame,
                owner: self.h,
                kind: Kind::TxNative(Native { ptr: f.data, linesize: f.linesize as usize, width: f.width as usize, lines }),
                sent: false,
            })
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

// ---------- La vidéo compressée : ST 2110-22 (JPEG XS) ----------

/// Le greffon JPEG XS de MTL (SVT-JPEG-XS), posé à côté de libmtl.
pub const JPEGXS_PLUGIN: &str = "libst_plugin_st22_svt_jpeg_xs.so";

/// Inscrit les greffons posés à côté de `libmtl` (JPEG XS) pour MTL : un registre écrit dans
/// `dir`, donné à MTL par `KAHAWAI_CFG_PATH`. À faire avant [`Mtl::open`] (MTL le lit en
/// s'ouvrant). Rend le greffon trouvé.
pub fn register_plugins(libmtl: &Path, dir: &Path) -> Option<PathBuf> {
    let plugin = libmtl.parent()?.join(JPEGXS_PLUGIN);
    if !plugin.is_file() {
        return None;
    }
    let cfg = dir.join("mtl-plugins.json");
    let body = format!("{{\"plugins\": [{{\"enabled\": 1, \"name\": \"st22_svt_jpeg_xs\", \"path\": {:?}}}]}}\n", plugin.display().to_string());
    std::fs::write(&cfg, body).ok()?;
    std::env::set_var("KAHAWAI_CFG_PATH", &cfg);
    Some(plugin)
}

/// Cette MTL fait-elle du ST 2110-22 ?
pub fn has_st22p() -> bool {
    #[cfg(mtl_shim)]
    // SAFETY: reads a flag set when the library was loaded.
    return unsafe { ffi::smtl_has_st22p() } != 0;
    #[cfg(not(mtl_shim))]
    false
}

/// Les octets d'une image JPEG XS (d'une trame en entrelacé) à `bpp` bits par pixel.
pub fn codestream_size(v: &Video, bpp: f32) -> u64 {
    let lines = if v.interlaced { v.height / 2 } else { v.height } as f64;
    (bpp.max(0.5) as f64 * v.width as f64 * lines / 8.0).round() as u64
}

#[inline]
fn put16(b: &mut [u8], i: usize, v: u32) {
    b[2 * i..2 * i + 2].copy_from_slice(&(v as u16).to_le_bytes());
}

#[inline]
fn get16(b: &[u8], i: usize) -> u32 {
    u16::from_le_bytes([b[2 * i], b[2 * i + 1]]) as u32
}

/// Le k-ième échantillon de 10 bits d'une ligne v210 (Cb Y Cr Y…, trois par mot de 32 bits).
#[inline]
fn v210_sample(line: &[u8], k: usize) -> u32 {
    let w = u32::from_le_bytes(line[4 * (k / 3)..4 * (k / 3) + 4].try_into().unwrap());
    (w >> (10 * (k % 3))) & 0x3ff
}

/// Une ligne RFC 4175 (4:2:2 10 bits gros-boutiste : Cb Y Cr Y en 5 octets pour 2 pixels) en
/// v210 (les mêmes composantes, trois par mot de 32 bits petit-boutiste ; la fin de ligne à 0).
pub fn rfc4175_to_v210(src: &[u8], dst: &mut [u8], width: usize) {
    let groups = width / 2;
    let (mut word, mut k, mut o) = (0u32, 0u32, 0usize);
    for g in src[..groups * 5].as_chunks::<5>().0 {
        let (b0, b1, b2, b3, b4) = (g[0] as u32, g[1] as u32, g[2] as u32, g[3] as u32, g[4] as u32);
        for c in [(b0 << 2) | (b1 >> 6), ((b1 & 0x3f) << 4) | (b2 >> 4), ((b2 & 0x0f) << 6) | (b3 >> 2), ((b3 & 0x03) << 8) | b4] {
            word |= c << (10 * k);
            k += 1;
            if k == 3 {
                dst[o..o + 4].copy_from_slice(&word.to_le_bytes());
                (word, k, o) = (0, 0, o + 4);
            }
        }
    }
    if k > 0 {
        dst[o..o + 4].copy_from_slice(&word.to_le_bytes());
        o += 4;
    }
    dst[o..].fill(0);
}

/// Une ligne v210 en RFC 4175 (l'inverse de [`rfc4175_to_v210`]).
pub fn v210_to_rfc4175(src: &[u8], dst: &mut [u8], width: usize) {
    let n = width / 2 * 4;
    let comp = |i: usize| {
        let w = u32::from_le_bytes(src[4 * (i / 3)..4 * (i / 3) + 4].try_into().unwrap());
        (w >> (10 * (i % 3))) & 0x3ff
    };
    for (g, d) in (0..n).step_by(4).zip(dst.as_chunks_mut::<5>().0) {
        let (c0, c1, c2, c3) = (comp(g), comp(g + 1), comp(g + 2), comp(g + 3));
        d[0] = (c0 >> 2) as u8;
        d[1] = ((c0 << 6) | (c1 >> 4)) as u8;
        d[2] = ((c1 << 4) | (c2 >> 6)) as u8;
        d[3] = ((c2 << 2) | (c3 >> 8)) as u8;
        d[4] = c3 as u8;
    }
}

#[inline]
fn v210_set(line: &mut [u8], k: usize, v: u32) {
    let at = 4 * (k / 3);
    let mut w = u32::from_le_bytes(line[at..at + 4].try_into().unwrap());
    let sh = 10 * (k % 3);
    w = (w & !(0x3ff << sh)) | ((v & 0x3ff) << sh);
    line[at..at + 4].copy_from_slice(&w.to_le_bytes());
}

/// Des lignes v210 (`stride` octets chacune) vers trois plans 4:2:2 (Y, Cb, Cr ; 10 bits dans 16,
/// petit-boutiste), le format du greffon JPEG XS.
pub fn v210_to_planar(src: &[u8], stride: usize, width: usize, lines: usize, planes: [&mut [u8]; 3], linesize: [usize; 3]) {
    let [py, pb, pr] = planes;
    let blocks = width / 6;
    for row in 0..lines {
        let s = &src[row * stride..row * stride + stride];
        let y = &mut py[row * linesize[0]..];
        let cb = &mut pb[row * linesize[1]..];
        let cr = &mut pr[row * linesize[2]..];
        // Six pixels (quatre mots) à la fois.
        for b in 0..blocks {
            let w: [u32; 4] = std::array::from_fn(|i| u32::from_le_bytes(s[16 * b + 4 * i..16 * b + 4 * i + 4].try_into().unwrap()));
            let x = |i: usize| (w[i / 3] >> (10 * (i % 3))) & 0x3ff;
            put16(cb, 3 * b, x(0));
            put16(y, 6 * b, x(1));
            put16(cr, 3 * b, x(2));
            put16(y, 6 * b + 1, x(3));
            put16(cb, 3 * b + 1, x(4));
            put16(y, 6 * b + 2, x(5));
            put16(cr, 3 * b + 1, x(6));
            put16(y, 6 * b + 3, x(7));
            put16(cb, 3 * b + 2, x(8));
            put16(y, 6 * b + 4, x(9));
            put16(cr, 3 * b + 2, x(10));
            put16(y, 6 * b + 5, x(11));
        }
        // Le reste (largeur pas multiple de 6 : 1280), une paire de pixels à la fois.
        for p in blocks * 3..width / 2 {
            put16(cb, p, v210_sample(s, 4 * p));
            put16(y, 2 * p, v210_sample(s, 4 * p + 1));
            put16(cr, p, v210_sample(s, 4 * p + 2));
            put16(y, 2 * p + 1, v210_sample(s, 4 * p + 3));
        }
    }
}

/// L'inverse : trois plans 4:2:2 10 bits vers des lignes v210.
pub fn planar_to_v210(planes: [&[u8]; 3], linesize: [usize; 3], width: usize, lines: usize, dst: &mut [u8], stride: usize) {
    let [py, pb, pr] = planes;
    let blocks = width / 6;
    for row in 0..lines {
        let d = &mut dst[row * stride..row * stride + stride];
        let y = &py[row * linesize[0]..];
        let cb = &pb[row * linesize[1]..];
        let cr = &pr[row * linesize[2]..];
        for b in 0..blocks {
            let s = [
                get16(cb, 3 * b),
                get16(y, 6 * b),
                get16(cr, 3 * b),
                get16(y, 6 * b + 1),
                get16(cb, 3 * b + 1),
                get16(y, 6 * b + 2),
                get16(cr, 3 * b + 1),
                get16(y, 6 * b + 3),
                get16(cb, 3 * b + 2),
                get16(y, 6 * b + 4),
                get16(cr, 3 * b + 2),
                get16(y, 6 * b + 5),
            ];
            for i in 0..4 {
                let w = (s[3 * i] & 0x3ff) | (s[3 * i + 1] & 0x3ff) << 10 | (s[3 * i + 2] & 0x3ff) << 20;
                d[16 * b + 4 * i..16 * b + 4 * i + 4].copy_from_slice(&w.to_le_bytes());
            }
        }
        for p in blocks * 3..width / 2 {
            v210_set(d, 4 * p, get16(cb, p));
            v210_set(d, 4 * p + 1, get16(y, 2 * p));
            v210_set(d, 4 * p + 2, get16(cr, p));
            v210_set(d, 4 * p + 3, get16(y, 2 * p + 1));
        }
    }
}

/// Le pas d'une ligne v210 (multiple de 128 octets, comme MXL).
fn v210_stride(width: u32) -> usize {
    (width as usize).div_ceil(48) * 128
}

impl Mtl {
    /// Reçoit un flux JPEG XS (ST 2110-22) : les images, décodées, en v210.
    pub fn rx22(&self, name: &str, legs: &[Leg], payload_type: u8, v: &Video) -> Result<Rx22, String> {
        let fps = st_fps(v).ok_or("MTL has no such frame rate")?;
        #[cfg(mtl_shim)]
        {
            let (idx, ips, udps, sips) = legs_of(legs);
            let n = CString::new(name).unwrap_or_default();
            let mut err = [0 as c_char; 256];
            let vv = ffi::Video { width: v.width, height: v.height, fps, interlaced: v.interlaced as c_int };
            // SAFETY: arrays of `legs.len()` entries; `err` sized as passed.
            let h = unsafe {
                ffi::smtl22_rx_open(
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
                return Err(ffi::text(&err));
            }
            Ok(Rx22 { h, _name: n, staging: Vec::new(), frames: 0 })
        }
        #[cfg(not(mtl_shim))]
        {
            let _ = (name, legs, payload_type, fps);
            Err(NOT_BUILT.into())
        }
    }

    /// Envoie un flux JPEG XS à `bpp` bits par pixel (4:2:2 10 bits non compressé : 20) ;
    /// `timing` comme pour [`Mtl::tx`].
    pub fn tx22(&self, name: &str, legs: &[Leg], payload_type: u8, v: &Video, bpp: f32, timing: Option<u32>) -> Result<Tx22, String> {
        let fps = st_fps(v).ok_or("MTL has no such frame rate")?;
        #[cfg(mtl_shim)]
        {
            let (idx, ips, udps, _) = legs_of(legs);
            let n = CString::new(name).unwrap_or_default();
            let mut err = [0 as c_char; 256];
            let vv = ffi::Video { width: v.width, height: v.height, fps, interlaced: v.interlaced as c_int };
            // SAFETY: arrays of `legs.len()` entries; `err` sized as passed.
            let h = unsafe {
                ffi::smtl22_tx_open(
                    self.h,
                    n.as_ptr(),
                    legs.len() as c_int,
                    idx.as_ptr(),
                    ips.as_ptr(),
                    udps.as_ptr(),
                    payload_type,
                    vv,
                    codestream_size(v, bpp),
                    timing.unwrap_or(0),
                    timing.is_some() as c_int,
                    err.as_mut_ptr(),
                    err.len() as c_int,
                )
            };
            if h.is_null() {
                return Err(ffi::text(&err));
            }
            Ok(Tx22 { h, _name: n, staging: Vec::new() })
        }
        #[cfg(not(mtl_shim))]
        {
            let _ = (name, legs, payload_type, fps, bpp, timing);
            Err(NOT_BUILT.into())
        }
    }
}

/// Un flux JPEG XS reçu.
pub struct Rx22 {
    #[cfg(mtl_shim)]
    h: *mut c_void,
    #[cfg(mtl_shim)]
    _name: CString,
    /// L'image décodée, remise en v210.
    #[cfg_attr(not(mtl_shim), allow(dead_code))]
    staging: Vec<u8>,
    /// Images reçues entières (MTL ne compte pas les paquets du 2110-22).
    frames: u64,
}

unsafe impl Send for Rx22 {}

impl Rx22 {
    /// La prochaine image, en v210 (200 ms au plus).
    pub fn frame(&mut self) -> Option<Frame<'_>> {
        #[cfg(mtl_shim)]
        {
            let mut f = ffi::Frame3::empty();
            // SAFETY: open session; `f` is a valid out pointer.
            if unsafe { ffi::smtl22_rx_get(self.h, &mut f) } != 1 {
                return None;
            }
            let (w, lines) = (f.width as usize, f.height as usize);
            let stride = v210_stride(f.width);
            self.staging.resize(stride * lines, 0);
            let ls = f.linesize.map(|x| x as usize);
            // SAFETY: MTL hands three planes of `lines` rows of `linesize` bytes, ours until put back.
            let planes: [&[u8]; 3] = std::array::from_fn(|i| unsafe { std::slice::from_raw_parts(f.data[i], ls[i] * lines) });
            planar_to_v210(planes, ls, w, lines, &mut self.staging, stride);
            if f.complete != 0 {
                self.frames += 1;
            }
            Some(Frame {
                data: &mut self.staging[..],
                linesize: stride,
                width: f.width,
                height: f.height,
                second_field: f.second_field != 0,
                complete: f.complete != 0,
                timestamp: f.timestamp,
                raw: f.frame,
                owner: self.h,
                kind: Kind::Rx22,
                sent: false,
            })
        }
        #[cfg(not(mtl_shim))]
        None
    }

    /// Les compteurs : MTL ne compte pas les paquets du 2110-22, les images entières seulement
    /// (pour chaque patte aussi : ce qui arrive, sans le détail par réseau).
    pub fn stats(&self) -> RxStats {
        RxStats { received: self.frames, legs: [(self.frames, 0); 2], ..Default::default() }
    }
}

impl Drop for Rx22 {
    fn drop(&mut self) {
        #[cfg(mtl_shim)]
        // SAFETY: opened by `rx22`, freed once.
        unsafe {
            ffi::smtl22_rx_close(self.h)
        };
    }
}

/// Un flux JPEG XS envoyé.
pub struct Tx22 {
    #[cfg(mtl_shim)]
    h: *mut c_void,
    #[cfg(mtl_shim)]
    _name: CString,
    /// L'image à remplir, en v210 ; convertie en l'envoyant.
    #[cfg_attr(not(mtl_shim), allow(dead_code))]
    staging: Vec<u8>,
}

unsafe impl Send for Tx22 {}

impl Tx22 {
    /// Une image à remplir en v210 (200 ms au plus), puis [`Frame::send_field`].
    pub fn frame(&mut self) -> Option<Frame<'_>> {
        #[cfg(mtl_shim)]
        {
            let mut f = ffi::Frame3::empty();
            // SAFETY: open session; `f` is a valid out pointer.
            if unsafe { ffi::smtl22_tx_get(self.h, &mut f) } != 1 {
                return None;
            }
            let lines = f.height as usize;
            let stride = v210_stride(f.width);
            self.staging.resize(stride * lines, 0);
            Some(Frame {
                data: &mut self.staging[..],
                linesize: stride,
                width: f.width,
                height: f.height,
                second_field: f.second_field != 0,
                complete: true,
                timestamp: 0,
                raw: f.frame,
                owner: self.h,
                kind: Kind::Tx22(Planes { ptr: f.data, linesize: f.linesize.map(|x| x as usize), width: f.width as usize, lines }),
                sent: false,
            })
        }
        #[cfg(not(mtl_shim))]
        None
    }
}

impl Drop for Tx22 {
    fn drop(&mut self) {
        #[cfg(mtl_shim)]
        // SAFETY: opened by `tx22`, freed once.
        unsafe {
            ffi::smtl22_tx_close(self.h)
        };
    }
}

/// Un émetteur vidéo, 2110-20 ou 2110-22 : une image v210 à remplir, puis envoyée.
pub trait VideoTx: Send {
    fn next_frame(&mut self) -> Option<Frame<'_>>;
}

impl VideoTx for Tx {
    fn next_frame(&mut self) -> Option<Frame<'_>> {
        self.frame()
    }
}

impl VideoTx for Tx22 {
    fn next_frame(&mut self) -> Option<Frame<'_>> {
        self.frame()
    }
}

/// Un récepteur vidéo, 2110-20 ou 2110-22 : les images en v210, et ses compteurs.
pub trait VideoRx: Send {
    fn next_frame(&mut self) -> Option<Frame<'_>>;
    fn counters(&self) -> RxStats;
}

impl VideoRx for Rx {
    fn next_frame(&mut self) -> Option<Frame<'_>> {
        self.frame()
    }
    fn counters(&self) -> RxStats {
        self.stats()
    }
}

impl VideoRx for Rx22 {
    fn next_frame(&mut self) -> Option<Frame<'_>> {
        self.frame()
    }
    fn counters(&self) -> RxStats {
        self.stats()
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
            ssrc: u32,
            user_time: c_int,
            err: *mut c_char,
            errlen: c_int,
        ) -> *mut c_void;
        pub fn smtl_atx_get(tx: *mut c_void, out: *mut AFrame) -> c_int;
        pub fn smtl_atx_put(tx: *mut c_void, frame: *mut c_void, tai_ns: u64) -> c_int;
        pub fn smtl_atx_close(tx: *mut c_void);
    }

    pub fn empty() -> AFrame {
        AFrame { frame: std::ptr::null_mut(), data: std::ptr::null_mut(), size: 0, timestamp: 0, complete: 0 }
    }
}

impl Mtl {
    #[cfg(mtl_shim)]
    fn audio_open(&self, tx: Option<Option<u32>>, name: &str, legs: &[Leg], payload_type: u8, a: &Audio) -> Result<(*mut c_void, CString), String> {
        let fmt = a.fmt().ok_or("ST 2110-30: 16 or 24-bit samples")?;
        let ptime = a.ptime().ok_or("ST 2110-30: 1 ms or 125 µs packets")?;
        let (idx, ips, udps, sips) = legs_of(legs);
        let n = CString::new(name).unwrap_or_default();
        let mut err = [0 as c_char; 256];
        let aa = affi::Audio { channels: a.channels, fmt, ptime, frame_ms: a.frame_ms.max(1) };
        // SAFETY: arrays of `legs.len()` entries; `err` sized as passed.
        let h = unsafe {
            match tx {
                // Envoi : `timing` (IPMX) comme pour la vidéo.
                Some(timing) => affi::smtl_atx_open(
                    self.h,
                    n.as_ptr(),
                    legs.len() as c_int,
                    idx.as_ptr(),
                    ips.as_ptr(),
                    udps.as_ptr(),
                    sips.as_ptr(),
                    payload_type,
                    aa,
                    timing.unwrap_or(0),
                    timing.is_some() as c_int,
                    err.as_mut_ptr(),
                    err.len() as c_int,
                ),
                None => affi::smtl_arx_open(
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
                ),
            }
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
            self.audio_open(None, name, legs, payload_type, a).map(|(h, n)| ARx { h, _name: n, audio: *a })
        }
        #[cfg(not(mtl_shim))]
        {
            let _ = (name, legs, payload_type, a);
            Err(NOT_BUILT.into())
        }
    }

    /// Envoie un flux audio.
    pub fn atx(&self, name: &str, legs: &[Leg], payload_type: u8, a: &Audio, timing: Option<u32>) -> Result<ATx, String> {
        #[cfg(mtl_shim)]
        {
            self.audio_open(Some(timing), name, legs, payload_type, a).map(|(h, n)| ATx { h, _name: n, audio: *a })
        }
        #[cfg(not(mtl_shim))]
        {
            let _ = (name, legs, payload_type, a, timing);
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
        self.next_stamped().map(|(pcm, _)| pcm)
    }

    /// Le prochain bloc et son horodatage RTP (48 kHz : son premier échantillon).
    pub fn next_stamped(&self) -> Option<(Vec<f32>, u32)> {
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
            Some((pcm, f.timestamp as u32))
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
        self.send_at(pcm, 0)
    }

    /// Comme [`ATx::send`], le bloc partant à l'instant TAI `tai_ns` (son horodatage RTP ;
    /// 0 : le rythme de MTL).
    pub fn send_at(&self, pcm: &[f32], tai_ns: u64) -> bool {
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
            unsafe { affi::smtl_atx_put(self.h, f.frame, tai_ns) == 0 }
        }
        #[cfg(not(mtl_shim))]
        {
            let _ = (pcm, tai_ns);
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
    fn rfc4175_and_v210() {
        // Deux pixels : Cb 0x200, Y 0x040, Cr 0x200, Y 0x3ac.
        let pg = [0x80, 0x04, 0x08, 0x03, 0xac];
        let mut v = vec![0xffu8; 16];
        rfc4175_to_v210(&pg, &mut v, 2);
        let w0 = u32::from_le_bytes(v[0..4].try_into().unwrap());
        assert_eq!((w0 & 0x3ff, (w0 >> 10) & 0x3ff, (w0 >> 20) & 0x3ff), (0x200, 0x040, 0x200));
        assert_eq!(u32::from_le_bytes(v[4..8].try_into().unwrap()), 0x3ac);
        assert!(v[8..].iter().all(|&b| b == 0), "end of line zeroed");
        // Aller-retour en 1280 (non multiple de 6).
        let w = 1280;
        let src: Vec<u8> = (0..w / 2 * 5).map(|i| (i * 37 % 251) as u8).collect();
        let mut line = vec![0u8; v210_stride(w as u32)];
        rfc4175_to_v210(&src, &mut line, w);
        let mut back = vec![0u8; w / 2 * 5];
        v210_to_rfc4175(&line, &mut back, w);
        assert_eq!(src, back);
    }

    /// v210 → plans → v210 : identique, largeur multiple de 6 ou non (1280).
    #[test]
    fn planar_round_trip() {
        for (w, lines) in [(1920usize, 3usize), (1280, 2), (720, 1)] {
            let stride = v210_stride(w as u32);
            let mut src = vec![0u8; stride * lines];
            let mut seed = 12345u32;
            for row in 0..lines {
                for k in 0..w * 2 {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    v210_set(&mut src[row * stride..], k, (seed >> 8) & 0x3ff);
                }
            }
            let ls = [w * 2 + 64, w + 32, w + 32];
            let (mut y, mut cb, mut cr) = (vec![0u8; ls[0] * lines], vec![0u8; ls[1] * lines], vec![0u8; ls[2] * lines]);
            v210_to_planar(&src, stride, w, lines, [&mut y, &mut cb, &mut cr], ls);
            // Le premier pixel : Cb0 Y0 Cr0 Y1.
            assert_eq!(
                (get16(&cb, 0), get16(&y, 0), get16(&cr, 0), get16(&y, 1)),
                (v210_sample(&src, 0), v210_sample(&src, 1), v210_sample(&src, 2), v210_sample(&src, 3))
            );
            let mut back = vec![0u8; stride * lines];
            planar_to_v210([&y, &cb, &cr], ls, w, lines, &mut back, stride);
            for row in 0..lines {
                for k in 0..w * 2 {
                    assert_eq!(v210_sample(&back[row * stride..], k), v210_sample(&src[row * stride..], k), "w {w} row {row} sample {k}");
                }
            }
        }
    }

    #[test]
    fn jpeg_xs_frame_size() {
        let v = Video { width: 1920, height: 1080, rate: (50, 1), interlaced: false };
        assert_eq!(codestream_size(&v, 2.0), 1920 * 1080 * 2 / 8);
        let i = Video { interlaced: true, ..v };
        assert_eq!(codestream_size(&i, 2.0), 1920 * 540 * 2 / 8);
    }

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
