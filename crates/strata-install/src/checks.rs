//! Ce que l'installateur regarde avant d'installer : le système (Ubuntu 24.04, processeur, mémoire,
//! disque, internet), le nom du serveur, les cartes Blackmagic et leur pilote, la bibliothèque NDI,
//! et une installation déjà là.
use std::{
    net::{IpAddr, ToSocketAddrs},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use crate::paths;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    /// Possible, mais à savoir.
    Warn,
    /// Bloquant : l'installation s'arrête là.
    Bad,
}

#[derive(Clone, Debug)]
pub struct Check {
    pub label: String,
    pub level: Level,
    pub detail: String,
}

fn check(label: &str, level: Level, detail: impl Into<String>) -> Check {
    Check { label: label.into(), level, detail: detail.into() }
}

/// `/etc/os-release` : (ID, VERSION_ID, PRETTY_NAME).
pub fn os_release(text: &str) -> (String, String, String) {
    let get = |k: &str| text.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).map(|v| v.trim().trim_matches('"').to_string()).unwrap_or_default();
    (get("ID"), get("VERSION_ID"), get("PRETTY_NAME"))
}

/// Mémoire totale (Go) d'après `/proc/meminfo`.
pub fn mem_gb(meminfo: &str) -> f64 {
    meminfo
        .lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))
        .and_then(|v| v.split_whitespace().next()?.parse::<f64>().ok())
        .map_or(0.0, |kb| kb / 1024.0 / 1024.0)
}

/// Place libre (Go) sur le disque qui porte `path` (ou son premier parent existant).
pub fn free_gb(path: &Path) -> Option<f64> {
    let mut p = path.to_path_buf();
    while !p.exists() {
        p = p.parent()?.to_path_buf();
    }
    let c = std::ffi::CString::new(p.as_os_str().to_string_lossy().as_bytes()).ok()?;
    // SAFETY: `st` est rempli par statvfs ; le chemin est une chaîne C valide.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    Some(st.f_bavail as f64 * st.f_frsize as f64 / 1e9)
}

fn internet() -> Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(8))).build().into();
    agent.get("https://api.github.com/").header("user-agent", "strata-install").call().map(|_| ()).map_err(|e| e.to_string())
}

/// Le système : ✓, ! ou ✗ pour chaque point. `offline` : une archive du Core est donnée.
pub fn system(offline: bool) -> Vec<Check> {
    let mut out = Vec::new();
    let (id, ver, pretty) = os_release(&std::fs::read_to_string("/etc/os-release").unwrap_or_default());
    out.push(match (id.as_str(), ver.as_str()) {
        ("ubuntu", "24.04") => check("Système", Level::Ok, pretty),
        ("ubuntu", _) => check("Système", Level::Warn, format!("{pretty} : prévu pour Ubuntu 24.04")),
        _ => check("Système", Level::Bad, format!("{} : Strata s'installe sur Ubuntu 24.04", if pretty.is_empty() { "inconnu".into() } else { pretty })),
    });
    out.push(if std::env::consts::ARCH == "x86_64" {
        check("Processeur", Level::Ok, format!("x86-64, {} cœurs", cores()))
    } else {
        check("Processeur", Level::Bad, format!("{} : x86-64 seulement", std::env::consts::ARCH))
    });
    if cores() < 4 {
        out.last_mut().unwrap().level = Level::Warn;
        out.last_mut().unwrap().detail = format!("{} cœurs : 4 au moins conseillés", cores());
    }
    out.push(cpu_features(&std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default(), virt().as_deref()));
    out.push(match Command::new("systemctl").args(["is-active", "--quiet", "systemd-networkd"]).status() {
        Ok(s) if s.success() => check("Réseau", Level::Ok, "systemd-networkd (adresses réglables depuis le cockpit)"),
        _ => check("Réseau", Level::Warn, "systemd-networkd inactif : les adresses IP ne se règlent pas depuis le cockpit"),
    });
    let mem = mem_gb(&std::fs::read_to_string("/proc/meminfo").unwrap_or_default());
    out.push(match mem {
        m if m >= 7.5 => check("Mémoire", Level::Ok, format!("{m:.0} Go")),
        m if m >= 3.5 => check("Mémoire", Level::Warn, format!("{m:.0} Go : 8 Go au moins conseillés")),
        m => check("Mémoire", Level::Bad, format!("{m:.1} Go : 4 Go au moins")),
    });
    out.push(match free_gb(Path::new(paths::DATA)) {
        Some(g) if g >= 20.0 => check("Disque", Level::Ok, format!("{g:.0} Go libres")),
        Some(g) if g >= 5.0 => check("Disque", Level::Warn, format!("{g:.0} Go libres : 20 Go au moins conseillés")),
        Some(g) => check("Disque", Level::Bad, format!("{g:.1} Go libres : 5 Go au moins")),
        None => check("Disque", Level::Warn, "place libre inconnue"),
    });
    out.push(if Path::new("/run/systemd/system").exists() {
        check("systemd", Level::Ok, "présent")
    } else {
        check("systemd", Level::Bad, "absent : le Core est un service systemd")
    });
    out.push(match internet() {
        Ok(()) => check("Internet", Level::Ok, "GitHub joignable"),
        Err(_) if offline => check("Internet", Level::Warn, "pas d'accès : installation depuis l'archive donnée"),
        Err(e) => check("Internet", Level::Bad, format!("GitHub injoignable ({e}) : donnez l'archive du Core (--package)")),
    });
    // SAFETY: geteuid n'a aucune précondition.
    out.push(if unsafe { libc::geteuid() } == 0 { check("Droits", Level::Ok, "root") } else { check("Droits", Level::Bad, "lancez l'installateur avec sudo") });
    out
}

/// La machine virtuelle, s'il y en a une (`systemd-detect-virt` : kvm, vmware, microsoft…).
fn virt() -> Option<String> {
    let o = Command::new("systemd-detect-virt").arg("--vm").output().ok()?;
    let v = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (o.status.success() && !v.is_empty() && v != "none").then_some(v)
}

/// Les jeux d'instructions : AVX2 (MXL, MTL, conversions d'images) ; dans une machine virtuelle,
/// le type de processeur « host » les donne.
pub fn cpu_features(cpuinfo: &str, virt: Option<&str>) -> Check {
    let flags: Vec<&str> =
        cpuinfo.lines().find_map(|l| l.strip_prefix("flags")).map(|l| l.trim_start_matches([' ', '\t', ':']).split_whitespace().collect()).unwrap_or_default();
    let has = |f: &str| flags.contains(&f);
    let model = cpuinfo.lines().find_map(|l| l.strip_prefix("model name")).map(|l| l.trim_start_matches([' ', '\t', ':']).to_string()).unwrap_or_default();
    let vm = virt.map(|v| format!(" (machine virtuelle {v})")).unwrap_or_default();
    match (has("avx2"), has("avx512f")) {
        (true, true) => check("Instructions", Level::Ok, format!("AVX2, AVX-512{vm}")),
        (true, false) => check("Instructions", Level::Ok, format!("AVX2{vm}")),
        _ if virt.is_some() => check(
            "Instructions",
            Level::Warn,
            format!("pas d'AVX2 ({model}) : réglez le type de processeur de la machine virtuelle sur « host » (ou x86-64-v3), sinon le ST 2110 et les images sont lents"),
        ),
        _ => check("Instructions", Level::Warn, format!("pas d'AVX2 ({model}) : le ST 2110 et les conversions d'images seront lents")),
    }
}

pub fn cores() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}

/// Les adresses IPv4 de la machine (hors boucle locale), via `hostname -I`.
pub fn local_ips() -> Vec<IpAddr> {
    Command::new("hostname")
        .arg("-I")
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .filter(|ip: &IpAddr| ip.is_ipv4() && !ip.is_loopback())
                .collect()
        })
        .unwrap_or_default()
}

/// Ce que donne le nom : à quelles adresses il mène, et si c'est cette machine.
pub fn dns(name: &str) -> Check {
    let ips: Vec<IpAddr> = (name, 80).to_socket_addrs().map(|a| a.map(|s| s.ip()).collect()).unwrap_or_default();
    let mine = local_ips();
    let first = mine.first().map(|i| i.to_string()).unwrap_or_else(|| "<adresse du serveur>".into());
    if ips.is_empty() {
        return check("Nom DNS", Level::Warn, format!("{name} n'est pas résolu : enregistrements à créer : {name} A {first}, *.{name} A {first}"));
    }
    if ips.iter().any(|i| mine.contains(i) || i.is_loopback()) {
        check("Nom DNS", Level::Ok, format!("{name} → {}", ips.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ")))
    } else {
        check(
            "Nom DNS",
            Level::Warn,
            format!("{name} mène à {} et pas à ce serveur ({first})", ips.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ")),
        )
    }
}

/// Un nom DNS valable : des mots de lettres, chiffres et tirets, séparés par des points.
pub fn valid_dns(name: &str) -> bool {
    let labels: Vec<&str> = name.split('.').collect();
    name.len() <= 200
        && labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

// ---------- Le matériel ----------

/// Les cartes Blackmagic sur le bus PCI (fabricant 0xbdbd).
pub fn blackmagic_cards() -> usize {
    std::fs::read_dir("/sys/bus/pci/devices")
        .map(|d| d.flatten().filter(|e| std::fs::read_to_string(e.path().join("vendor")).is_ok_and(|v| v.trim() == "0xbdbd")).count())
        .unwrap_or(0)
}

/// Le pilote Desktop Video installé : sa version.
pub fn desktop_video() -> Option<String> {
    let out = Command::new("dpkg-query").args(["-W", "-f=${Version}", "desktopvideo"]).output().ok()?;
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !v.is_empty()).then_some(v)
}

/// Une archive Desktop Video déjà copiée sur le serveur (la plus récente).
/// Ce que Blackmagic donne à télécharger, ou ce qui en sort : l'archive (.tar.gz, .tgz, .tar),
/// son dossier déballé, le paquet .deb, un installateur .sh / .run.
pub fn is_desktop_video(name: &str, dir: bool) -> bool {
    let n = name.to_lowercase().replace(['-', ' '], "_");
    if !(n.contains("desktop_video") || n.contains("desktopvideo")) || n.contains("uninstall") || n.contains("gui") {
        return false;
    }
    dir || [".tar.gz", ".tgz", ".tar", ".deb", ".sh", ".run"].iter().any(|x| n.ends_with(x))
}

/// Un fichier que root peut lancer ou installer : à root ou à l'administrateur qui a lancé
/// l'installateur (`sudo`), et que personne d'autre ne peut modifier (ni lui, ni son dossier).
/// Un fichier déposé par un autre compte (une appli, un utilisateur, dans /tmp) est ignoré.
pub fn trusted(p: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let me = std::env::var("SUDO_UID").ok().and_then(|u| u.parse::<u32>().ok());
    let ok = |m: &std::fs::Metadata| (m.uid() == 0 || Some(m.uid()) == me) && m.mode() & 0o022 == 0;
    let Ok(m) = std::fs::symlink_metadata(p) else { return false };
    if m.file_type().is_symlink() || !ok(&m) {
        return false;
    }
    // Son dossier : pas modifiable par d'autres, sauf collant (/tmp) — le fichier y reste alors à
    // son propriétaire.
    match p.parent().and_then(|d| std::fs::metadata(d).ok()) {
        Some(d) => d.mode() & 0o1000 != 0 || ok(&d),
        None => false,
    }
}

pub fn find_desktop_video() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = vec![std::env::current_dir().unwrap_or_default(), "/root".into(), "/tmp".into()];
    if let Ok(home) = std::fs::read_dir("/home") {
        dirs.extend(home.flatten().map(|e| e.path()));
    }
    let mut found: Vec<PathBuf> = Vec::new();
    for d in dirs {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            // Un fichier encore en train d'arriver (copie pas finie) attend : rien n'y a été écrit
            // depuis 5 s.
            let settled =
                e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|d| d > std::time::Duration::from_secs(5));
            if is_desktop_video(&e.file_name().to_string_lossy(), e.path().is_dir()) && (settled || e.path().is_dir()) && trusted(&e.path()) {
                found.push(e.path());
            }
        }
    }
    found.sort_by_key(|p| p.file_name().map(|n| n.to_os_string()));
    found.pop()
}

/// Les GPU sur le bus PCI : (fabricant `nvidia` / `intel` / `amd`, pilote du noyau en place).
pub fn gpus() -> Vec<(&'static str, Option<String>)> {
    let mut out = Vec::new();
    for e in std::fs::read_dir("/sys/bus/pci/devices").into_iter().flatten().flatten() {
        let d = e.path();
        if !std::fs::read_to_string(d.join("class")).is_ok_and(|c| c.trim().starts_with("0x03")) {
            continue;
        }
        let vendor = match std::fs::read_to_string(d.join("vendor")).unwrap_or_default().trim() {
            "0x10de" => "nvidia",
            "0x8086" => "intel",
            "0x1002" => "amd",
            _ => continue,
        };
        let driver = std::fs::read_link(d.join("driver")).ok().and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()));
        out.push((vendor, driver));
    }
    out
}

/// Ce qu'il faut pour que les GPU encodent : (pilote NVIDIA à installer, paquets VA-API).
pub fn gpu_needs(gpus: &[(&'static str, Option<String>)]) -> (bool, Vec<String>) {
    let nvidia = gpus.iter().any(|(v, d)| *v == "nvidia" && d.as_deref() != Some("nvidia"));
    let mut pkgs: Vec<String> = Vec::new();
    if gpus.iter().any(|(v, _)| *v == "intel") {
        pkgs.extend(["intel-media-va-driver-non-free", "vainfo"].map(String::from));
    }
    if gpus.iter().any(|(v, _)| *v == "amd") {
        pkgs.extend(["mesa-va-drivers", "vainfo"].map(String::from));
    }
    pkgs.dedup();
    if !pkgs.is_empty() && Command::new("vainfo").arg("--version").output().is_ok_and(|o| o.status.success()) {
        pkgs.clear();
    }
    (nvidia, pkgs)
}

/// Secure Boot actif : un module ajouté (Blackmagic, NVIDIA) doit être signé par une clé enrôlée.
pub fn secure_boot() -> bool {
    Command::new("mokutil").arg("--sb-state").output().is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("SecureBoot enabled"))
}

/// Le SDK NDI copié à la main (serveur sans internet) : `Install_NDI_SDK_v6_Linux.tar.gz`, ou le
/// `.sh` qu'il contient, dans le dossier personnel, /root ou /tmp.
pub fn find_ndi_sdk() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = vec![std::env::current_dir().unwrap_or_default(), "/root".into(), "/tmp".into()];
    if let Ok(home) = std::fs::read_dir("/home") {
        dirs.extend(home.flatten().map(|e| e.path()));
    }
    let mut found: Vec<PathBuf> = Vec::new();
    for d in dirs {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().to_lowercase();
            let settled =
                e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|d| d > std::time::Duration::from_secs(5));
            if n.starts_with("install_ndi_sdk") && n.contains("linux") && (n.ends_with(".tar.gz") || n.ends_with(".sh")) && settled && trusted(&e.path()) {
                found.push(e.path());
            }
        }
    }
    found.sort_by_key(|p| p.file_name().map(|n| n.to_os_string()));
    found.pop()
}

/// La bibliothèque NDI installée.
pub fn ndi() -> Option<PathBuf> {
    ["/usr/local/lib/libndi.so", "/usr/lib/libndi.so", "/usr/lib/x86_64-linux-gnu/libndi.so"].iter().map(PathBuf::from).find(|p| p.exists())
}

/// La version du Core installé (`strata-core 0.3.0`), s'il l'est.
pub fn installed_core(root: &Path) -> Option<String> {
    // Une installation interrompue avant les réglages (pas de config.json) est reprise du début.
    if !paths::under(root, paths::DATA).join("config.json").exists() {
        return None;
    }
    let bin = paths::under(root, paths::BIN);
    // Le programme est au compte du service : root ne le lance pas lui-même.
    let out = if root == Path::new("/") && unsafe { libc::geteuid() } == 0 {
        Command::new("runuser").args(["-u", paths::USER, "--"]).arg(&bin).arg("--version").output().ok()?
    } else {
        Command::new(&bin).arg("--version").output().ok()?
    };
    let v = String::from_utf8_lossy(&out.stdout).split_whitespace().last().unwrap_or_default().to_string();
    (out.status.success() && !v.is_empty()).then_some(v)
}

/// Un dossier temporaire à soi : nom imprévisible, créé neuf (pas un dossier posé d'avance par un
/// autre compte), 0700.
pub fn private_tmp(prefix: &str) -> Result<PathBuf, String> {
    use std::io::Read;
    use std::os::unix::fs::DirBuilderExt;
    let mut r = [0u8; 8];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut r)).map_err(|e| e.to_string())?;
    let dir = std::env::temp_dir().join(format!("{prefix}-{}", r.iter().map(|b| format!("{b:02x}")).collect::<String>()));
    std::fs::DirBuilder::new().mode(0o700).create(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {

    #[test]
    fn cpu_features_in_a_vm() {
        let qemu = "model name\t: QEMU Virtual CPU version 2.5+\nflags\t\t: fpu sse sse2 sse4_1 sse4_2\n";
        let c = super::cpu_features(qemu, Some("kvm"));
        assert_eq!(c.level, super::Level::Warn);
        assert!(c.detail.contains("host"), "{}", c.detail);
        let host = "model name\t: Intel(R) Core(TM) i9-9900X\nflags\t\t: sse4_2 avx2 avx512f avx512bw\n";
        assert_eq!(super::cpu_features(host, Some("kvm")).level, super::Level::Ok);
    }

    use super::*;

    #[test]
    fn recognises_the_blackmagic_download() {
        assert!(is_desktop_video("Blackmagic_Desktop_Video_Linux_15.0.tar.gz", false));
        assert!(is_desktop_video("Blackmagic_Desktop_Video_Linux_15.0", true));
        assert!(is_desktop_video("desktopvideo_14.4.1a4_amd64.deb", false));
        assert!(is_desktop_video("Blackmagic Desktop Video Linux 15.0.sh", false));
        assert!(!is_desktop_video("desktopvideo-gui_14.4_amd64.deb", false));
        assert!(!is_desktop_video("Blackmagic_Desktop_Video_Linux_15.0.zip", false));
        assert!(!is_desktop_video("notes.txt", false));
    }

    #[test]
    fn reads_the_system() {
        let (id, v, p) = os_release("NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nID=ubuntu\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\n");
        assert_eq!((id.as_str(), v.as_str(), p.as_str()), ("ubuntu", "24.04", "Ubuntu 24.04.1 LTS"));
        assert!((mem_gb("MemTotal:       16303860 kB\nMemFree: 1 kB\n") - 15.55).abs() < 0.01);
        assert!(free_gb(Path::new("/tmp/does/not/exist")).is_some());
        assert!(valid_dns("strata01.regie.fr") && !valid_dns("strata01") && !valid_dns("Strata.fr") && !valid_dns("a..b"));
    }
}
