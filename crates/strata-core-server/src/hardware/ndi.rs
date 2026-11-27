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
    if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, &text)) {
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

struct Lib {
    _lib: libloading::Library,
    path: String,
    version: Option<String>,
    find_create: unsafe extern "C" fn(*const FindCreate) -> Instance,
    find_wait: unsafe extern "C" fn(Instance, u32) -> bool,
    find_sources: unsafe extern "C" fn(Instance, *mut u32) -> *const Source,
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
}
