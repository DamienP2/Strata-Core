//! Ce qu'il faut au système pour que le Core tienne les flux ST 2110 avec MTL :
//!
//! - **Hugepages** : MTL (DPDK) range ses images dedans, même par le noyau ou AF_XDP.
//! - **DPDK** : les ports média quittent le noyau pour `vfio-pci` (IOMMU nécessaire), et leur
//!   groupe VFIO passe au compte du service.
//!
//! Le Core (sans droits) écrit ce qu'il veut dans `/etc/strata/media.json` et relance
//! `strata-media.service` (autorisé par polkit) ; ce service, en root, lance
//! `strata-core media-setup`, qui fait le travail et laisse son bilan dans
//! `/run/strata-media.json`. Au démarrage du serveur, il repasse avant le Core.
//!
//! Garde-fous du côté root : seuls des ports Ethernet sont touchés, jamais celui qui porte la
//! route par défaut ; seuls les ports que Strata a passés en DPDK sont rendus au noyau ; les
//! hugepages ne dépassent pas la moitié de la mémoire.
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use super::{net, parse_format, Essence, HardwareConfig, MediaIo};

/// Pages de 2 Mo.
const PAGE: u64 = 2 * 1024 * 1024;

/// Ce que le Core demande.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Wanted {
    /// Pages de 2 Mo à réserver (0 : rendre celles réservées).
    pub hugepages: u64,
    /// Les ports à passer en DPDK.
    #[serde(default)]
    pub dpdk: Vec<DpdkPort>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct DpdkPort {
    /// Son nom quand le noyau le tenait (`ens1f0`).
    pub iface: String,
    /// `0000:af:00.0`.
    pub pci: String,
}

/// Le bilan du service root.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub ok: bool,
    pub hugepages: u64,
    #[serde(default)]
    pub dpdk: Vec<String>,
    pub error: Option<String>,
}

fn etc() -> PathBuf {
    std::env::var_os("STRATA_MEDIA_FILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/etc/strata/media.json"))
}

fn outcome_path() -> PathBuf {
    std::env::var_os("STRATA_MEDIA_OUTCOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/run/strata-media.json"))
}

/// Ce qu'il faut d'après les réglages (rien en mode « Off »).
pub fn wanted(cfg: &HardwareConfig) -> Wanted {
    let r = &cfg.network;
    let video: Vec<_> = cfg.flows.iter().filter(|f| f.essence == Essence::Video).collect();
    if r.media_io == MediaIo::Off || video.is_empty() {
        return Wanted::default();
    }
    // 2 Go de base, et par flux : 512 Mo en HD, 2 Go en UHD.
    let mut pages = 1024;
    for f in &video {
        let h = parse_format(&f.format).map_or(1080, |v| v.1);
        pages += if h > 1080 { 1024 } else { 256 };
    }
    let mut dpdk = Vec::new();
    if r.media_io == MediaIo::Dpdk {
        let known = read_wanted();
        for iface in [&r.media1, &r.media2] {
            if iface.is_empty() || dpdk.iter().any(|p: &DpdkPort| &p.iface == iface) {
                continue;
            }
            if let Some(pci) = pci_of(iface, &known) {
                dpdk.push(DpdkPort { iface: iface.clone(), pci });
            }
        }
    }
    Wanted { hugepages: pages, dpdk }
}

/// L'adresse PCI d'un port, même déjà passé en DPDK (le noyau ne le voit plus : on la garde).
pub fn pci_of(iface: &str, known: &Wanted) -> Option<String> {
    net::nics()
        .into_iter()
        .find(|n| n.name == iface)
        .and_then(|n| n.pci.map(|p| format!("{p}.{}", n.port.unwrap_or(0))))
        .or_else(|| known.dpdk.iter().find(|p| p.iface == iface).map(|p| p.pci.clone()))
}

pub fn read_wanted() -> Wanted {
    std::fs::read_to_string(etc()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn read_outcome() -> Option<Outcome> {
    std::fs::read_to_string(outcome_path()).ok().and_then(|s| serde_json::from_str(&s).ok())
}

/// Ce qui a été demandé la dernière fois, et sa réponse.
static LAST: Mutex<Option<(Wanted, Result<(), String>)>> = Mutex::new(None);

/// Côté Core : obtient du système ce qu'il faut (si ça a changé) ; Err : pourquoi MTL ne
/// pourra pas s'ouvrir.
pub fn prepare(cfg: &HardwareConfig) -> Result<(), String> {
    let want = wanted(cfg);
    let mut last = LAST.lock().unwrap();
    if let Some((w, r)) = last.as_ref() {
        if *w == want {
            return r.clone();
        }
    }
    let r = ask(&want);
    if let Err(e) = &r {
        tracing::warn!("media setup: {e}");
    }
    *last = Some((want, r.clone()));
    r
}

fn ask(want: &Wanted) -> Result<(), String> {
    // Hors d'un serveur installé (développement) : rien à demander, MTL dira ce qui manque.
    if !Path::new("/etc/systemd/system/strata-media.service").exists() {
        return Ok(());
    }
    let current = read_wanted();
    let applied = read_outcome().is_some_and(|o| o.ok && o.hugepages >= want.hugepages && o.dpdk.len() == want.dpdk.len());
    if current == *want && applied {
        return Ok(());
    }
    // Rien n'est demandé et rien n'a jamais été fait : rien à toucher.
    if *want == Wanted::default() && current == Wanted::default() {
        return Ok(());
    }
    let json = serde_json::to_string_pretty(want).unwrap_or_default();
    std::fs::write(etc(), json).map_err(|e| format!("cannot write {}: {e}", etc().display()))?;
    if let Some(e) = super::clock::run("systemctl", &["restart", "strata-media"]) {
        // Le bilan dit mieux pourquoi.
        return Err(read_outcome().and_then(|o| o.error).unwrap_or(format!("strata-media: {e}")));
    }
    match read_outcome() {
        Some(o) if o.ok => Ok(()),
        Some(o) => Err(o.error.unwrap_or("the media setup failed".into())),
        None => Err("strata-media gave no outcome".into()),
    }
}

// ---------- Côté root : `strata-core media-setup` ----------

/// Le service `strata-media` (posé par l'installateur).
pub const UNIT: &str = "[Unit]\n\
Description=Strata: hugepages and DPDK ports for ST 2110\n\
Before=strata-core.service\n\
After=systemd-modules-load.service\n\
\n\
[Service]\n\
Type=oneshot\n\
RemainAfterExit=yes\n\
ExecStart=/opt/strata/bin/strata-core media-setup\n\
\n\
[Install]\n\
WantedBy=multi-user.target\n";

/// Applique `/etc/strata/media.json` ; écrit le bilan ; Err si quelque chose a manqué.
pub fn setup(user: &str) -> Result<(), String> {
    let want = read_wanted();
    let sys = SysRoot::from_env();
    let r = setup_in(&sys, &want, user);
    let out = match &r {
        Ok((pages, dpdk)) => Outcome { ok: true, hugepages: *pages, dpdk: dpdk.clone(), error: None },
        Err(e) => Outcome { ok: false, hugepages: sys.hugepages(), dpdk: Vec::new(), error: Some(e.clone()) },
    };
    let _ = std::fs::write(outcome_path(), serde_json::to_string_pretty(&out).unwrap_or_default());
    r.map(|(pages, dpdk)| {
        println!("hugepages: {pages} × 2 MB; DPDK: {}", if dpdk.is_empty() { "none".to_string() } else { dpdk.join(", ") });
    })
}

/// `/sys`, `/proc` et `/dev` (sous un autre dossier pour les essais).
struct SysRoot(PathBuf);

impl SysRoot {
    fn from_env() -> SysRoot {
        SysRoot(std::env::var_os("STRATA_SYSTEM_ROOT").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/")))
    }

    fn p(&self, p: &str) -> PathBuf {
        self.0.join(p.trim_start_matches('/'))
    }

    fn read(&self, p: &str) -> Option<String> {
        std::fs::read_to_string(self.p(p)).ok().map(|s| s.trim().to_string())
    }

    fn write(&self, p: &str, v: &str) -> Result<(), String> {
        std::fs::write(self.p(p), v).map_err(|e| format!("{p}: {e}"))
    }

    fn hugepages(&self) -> u64 {
        self.read("/proc/sys/vm/nr_hugepages").and_then(|v| v.parse().ok()).unwrap_or(0)
    }

    fn mem_total(&self) -> u64 {
        self.read("/proc/meminfo")
            .and_then(|m| m.lines().find(|l| l.starts_with("MemTotal:")).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok()))
            .map_or(0, |kb| kb * 1024)
    }

    fn driver(&self, pci: &str) -> Option<String> {
        std::fs::read_link(self.p(&format!("/sys/bus/pci/devices/{pci}/driver"))).ok().and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
    }
}

/// Les hugepages réservées avant Strata : rendues telles quelles quand on n'en veut plus.
const BASELINE: &str = "/run/strata-media.baseline";

fn setup_in(sys: &SysRoot, want: &Wanted, user: &str) -> Result<(u64, Vec<String>), String> {
    // Les hugepages.
    let now = sys.hugepages();
    let baseline_file = sys.p(BASELINE);
    let baseline: u64 = std::fs::read_to_string(&baseline_file).ok().and_then(|v| v.trim().parse().ok()).unwrap_or_else(|| {
        let _ = std::fs::write(&baseline_file, now.to_string());
        now
    });
    let cap = sys.mem_total() / 2 / PAGE;
    let target = if want.hugepages == 0 { baseline } else { baseline.max(want.hugepages.min(cap.max(1))) };
    if target != now {
        sys.write("/proc/sys/vm/nr_hugepages", &target.to_string())?;
    }
    let got = sys.hugepages();
    if want.hugepages > 0 && got < target {
        return Err(format!("only {got} of the {target} hugepages could be reserved (memory too fragmented: restart the server)"));
    }
    // Les ports DPDK.
    let keep: Vec<&str> = want.dpdk.iter().map(|p| p.pci.as_str()).collect();
    release_others(sys, &keep)?;
    let mut bound = Vec::new();
    if !want.dpdk.is_empty() {
        if std::fs::read_dir(sys.p("/sys/kernel/iommu_groups")).map_or(true, |mut d| d.next().is_none()) {
            return Err("the IOMMU is off: add intel_iommu=on iommu=pt to the kernel command line (GRUB_CMDLINE_LINUX in /etc/default/grub, then update-grub) and restart the server".into());
        }
        let _ = super::clock::run("modprobe", &["vfio-pci"]);
        let main = default_route_iface(sys);
        for p in &want.dpdk {
            bind_vfio(sys, p, main.as_deref())?;
            give_group(sys, &p.pci, user)?;
            bound.push(format!("{} ({})", p.iface, p.pci));
        }
    }
    Ok((got, bound))
}

fn valid_pci(pci: &str) -> bool {
    let b = pci.as_bytes();
    b.len() == 12 && b[4] == b':' && b[7] == b':' && b[10] == b'.' && pci.chars().enumerate().all(|(i, c)| [4, 7, 10].contains(&i) || c.is_ascii_hexdigit())
}

/// Ethernet (classe PCI 0x02).
fn is_ethernet(sys: &SysRoot, pci: &str) -> bool {
    sys.read(&format!("/sys/bus/pci/devices/{pci}/class")).is_some_and(|c| c.starts_with("0x02"))
}

/// Le port qui porte la route par défaut : jamais en DPDK (le serveur ne répondrait plus).
fn default_route_iface(sys: &SysRoot) -> Option<String> {
    sys.read("/proc/net/route")?.lines().skip(1).find_map(|l| {
        let mut f = l.split_whitespace();
        let iface = f.next()?;
        (f.next()? == "00000000").then(|| iface.to_string())
    })
}

fn bind_vfio(sys: &SysRoot, p: &DpdkPort, main: Option<&str>) -> Result<(), String> {
    if !valid_pci(&p.pci) || !is_ethernet(sys, &p.pci) {
        return Err(format!("{}: not a network port", p.pci));
    }
    if sys.driver(&p.pci).as_deref() == Some("vfio-pci") {
        return Ok(());
    }
    // Le port tient-il la route par défaut ?
    if let Some(m) = main {
        if sys.p(&format!("/sys/bus/pci/devices/{}/net/{m}", p.pci)).exists() {
            return Err(format!("{} carries the server's default route: it cannot go to DPDK", p.iface));
        }
    }
    let dev = format!("/sys/bus/pci/devices/{}", p.pci);
    sys.write(&format!("{dev}/driver_override"), "vfio-pci")?;
    if sys.driver(&p.pci).is_some() {
        sys.write(&format!("{dev}/driver/unbind"), &p.pci)?;
    }
    sys.write("/sys/bus/pci/drivers_probe", &p.pci)?;
    match sys.driver(&p.pci).as_deref() {
        Some("vfio-pci") => Ok(()),
        _ => Err(format!("{}: vfio-pci did not take it", p.iface)),
    }
}

/// Le groupe VFIO du port au compte du service.
fn give_group(sys: &SysRoot, pci: &str, user: &str) -> Result<(), String> {
    let group = std::fs::read_link(sys.p(&format!("/sys/bus/pci/devices/{pci}/iommu_group"))).ok().and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()));
    let Some(g) = group else { return Err(format!("{pci}: no IOMMU group")) };
    let node = sys.p(&format!("/dev/vfio/{g}"));
    if sys.0 == Path::new("/") {
        if let Some(e) = super::clock::run("chown", &[user, &node.display().to_string()]) {
            return Err(format!("{}: {e}", node.display()));
        }
    }
    Ok(())
}

/// Rend au noyau les ports que Strata avait passés en DPDK et qu'on ne veut plus.
fn release_others(sys: &SysRoot, keep: &[&str]) -> Result<(), String> {
    let Ok(dir) = std::fs::read_dir(sys.p("/sys/bus/pci/devices")) else { return Ok(()) };
    for d in dir.flatten() {
        let pci = d.file_name().to_string_lossy().into_owned();
        if keep.contains(&pci.as_str()) || !is_ethernet(sys, &pci) {
            continue;
        }
        let dev = format!("/sys/bus/pci/devices/{pci}");
        if sys.read(&format!("{dev}/driver_override")).as_deref() != Some("vfio-pci") {
            continue;
        }
        sys.write(&format!("{dev}/driver_override"), "\n")?;
        if sys.driver(&pci).is_some() {
            sys.write(&format!("{dev}/driver/unbind"), &pci)?;
        }
        sys.write("/sys/bus/pci/drivers_probe", &pci)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pci_addresses_are_checked() {
        assert!(valid_pci("0000:af:00.0"));
        assert!(!valid_pci("0000:af:00"));
        assert!(!valid_pci("../../../x.0"));
    }

    #[test]
    fn hugepages_follow_the_flows_and_are_capped() {
        let dir = std::env::temp_dir().join(format!("strata-media-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("proc/sys/vm")).unwrap();
        std::fs::create_dir_all(dir.join("run")).unwrap();
        std::fs::write(dir.join("proc/sys/vm/nr_hugepages"), "16\n").unwrap();
        // 4 Go de mémoire : 1024 pages au plus.
        std::fs::write(dir.join("proc/meminfo"), "MemTotal:        4194304 kB\n").unwrap();
        let sys = SysRoot(dir.clone());
        let (got, _) = setup_in(&sys, &Wanted { hugepages: 2048, dpdk: vec![] }, "strata").unwrap();
        assert_eq!(got, 1024);
        // Plus rien de demandé : on rend ce qu'il y avait avant.
        let (got, _) = setup_in(&sys, &Wanted::default(), "strata").unwrap();
        assert_eq!(got, 16);
        // Sans IOMMU, le DPDK est refusé avec la marche à suivre.
        let e = setup_in(&sys, &Wanted { hugepages: 512, dpdk: vec![DpdkPort { iface: "ens1f0".into(), pci: "0000:af:00.0".into() }] }, "strata").unwrap_err();
        assert!(e.contains("intel_iommu=on"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
