//! ST 2110-20 (vidéo non compressée) par Intel Media Transport Library (MTL), sur les ports média
//! du serveur (Intel E810 : DPDK, AF_XDP, ou socket du noyau).
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
        pub fn smtl_rx_open(
            h: *mut c_void,
            name: *const c_char,
            n: c_int,
            port_index: *const c_int,
            ip: *const [u8; 4],
            udp: *const u16,
            pt: u8,
            v: Video,
            err: *mut c_char,
            errlen: c_int,
        ) -> *mut c_void;
        pub fn smtl_rx_get(rx: *mut c_void, out: *mut Frame) -> c_int;
        pub fn smtl_rx_put(rx: *mut c_void, frame: *mut c_void);
        pub fn smtl_rx_stats(rx: *mut c_void, received: *mut u64, lost: *mut u64);
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
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.join("../lib/mtl/libmtl.so"))) {
        v.push(dir);
    }
    // Livrée avec le Core (scripts/build-mtl.sh) : DPDK y cherche aussi ses pilotes.
    v.extend(["/opt/strata/lib/mtl/libmtl.so", "/usr/local/lib/x86_64-linux-gnu/libmtl.so", "/usr/local/lib/libmtl.so", "libmtl.so"].map(PathBuf::from));
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
            let (idx, ips, udps) = legs_of(legs);
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
    pub fn tx(&self, name: &str, legs: &[Leg], payload_type: u8, v: &Video) -> Result<Tx, String> {
        let fps = st_fps(v).ok_or("MTL has no such frame rate")?;
        #[cfg(mtl_shim)]
        {
            let (idx, ips, udps) = legs_of(legs);
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
            let _ = (name, legs, payload_type, fps);
            Err(NOT_BUILT.into())
        }
    }
}

#[cfg(mtl_shim)]
fn legs_of(legs: &[Leg]) -> (Vec<c_int>, Vec<[u8; 4]>, Vec<u16>) {
    (legs.iter().map(|l| l.port as c_int).collect(), legs.iter().map(|l| l.ip.octets()).collect(), legs.iter().map(|l| l.udp).collect())
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

    /// Paquets reçus, perdus.
    pub fn stats(&self) -> (u64, u64) {
        #[cfg(mtl_shim)]
        {
            let (mut r, mut l) = (0u64, 0u64);
            // SAFETY: open session; valid out pointers.
            unsafe { ffi::smtl_rx_stats(self.h, &mut r, &mut l) };
            (r, l)
        }
        #[cfg(not(mtl_shim))]
        (0, 0)
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
}
