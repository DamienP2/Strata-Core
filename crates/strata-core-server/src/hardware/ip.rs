//! Les adresses IP des interfaces, réglées depuis *Matériel › Réseau*.
//!
//! Le Core (sans droits) écrit ce qu'il veut dans `/etc/strata/network.json` et relance
//! `strata-net.service` (autorisé par polkit). Ce service, en root, lance `strata-core net-setup`,
//! qui écrit un fichier systemd-networkd par interface réglée
//! (`/etc/systemd/network/05-strata-<interface>.network`). Ces fichiers passent avant ceux de
//! netplan (`10-netplan-…`) : ceux d'Ubuntu ne sont jamais touchés, et retirer le nôtre rend
//! l'interface à sa configuration d'origine.
//!
//! Garde-fou : quand le changement touche l'interface par laquelle on joint le serveur, l'état
//! d'avant revient de lui-même au bout de 2 minutes (`strata-net-revert.timer`), sauf si on le
//! confirme depuis la nouvelle adresse.
use serde::{Deserialize, Serialize};
use std::{
    net::Ipv4Addr,
    path::{Path, PathBuf},
    time::Duration,
};

/// Le délai pour confirmer un changement risqué.
pub const REVERT_SECS: u64 = 120;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Par adresse(s) fixe(s).
    #[default]
    Static,
    Dhcp,
}

/// Les réglages d'une interface.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct IfaceIp {
    pub name: String,
    pub mode: Mode,
    /// « 10.0.0.2/24 ».
    pub addresses: Vec<String>,
    pub gateway: Option<String>,
    pub dns: Vec<String>,
    pub mtu: Option<u32>,
}

/// Ce que le Core demande : les interfaces réglées par Strata (les autres restent au système).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Wanted {
    pub interfaces: Vec<IfaceIp>,
    /// Retour arrière automatique si personne ne confirme.
    pub guard: bool,
}

/// Le bilan du service root.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Outcome {
    pub ok: bool,
    pub error: Option<String>,
    /// Retour arrière prévu (secondes Unix), si le changement est à confirmer.
    pub revert_at: Option<u64>,
    /// L'état d'avant a été remis (personne n'a confirmé, ou on l'a demandé).
    pub reverted: bool,
}

fn etc() -> PathBuf {
    std::env::var_os("STRATA_NET_FILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/etc/strata/network.json"))
}

fn prev_json() -> PathBuf {
    etc().with_file_name("network.prev.json")
}

fn outcome_path() -> PathBuf {
    std::env::var_os("STRATA_NET_OUTCOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/run/strata-net.json"))
}

fn networkd_dir() -> PathBuf {
    std::env::var_os("STRATA_NETWORKD_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/etc/systemd/network"))
}

pub fn read_wanted() -> Wanted {
    std::fs::read_to_string(etc()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn read_outcome() -> Option<Outcome> {
    std::fs::read_to_string(outcome_path()).ok().and_then(|s| serde_json::from_str(&s).ok())
}

// ---------- Vérifications (des deux côtés) ----------

fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 15 && n.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) && !n.starts_with('.')
}

/// « a.b.c.d/n » : l'adresse et la longueur du préfixe.
pub fn parse_cidr(s: &str) -> Option<(Ipv4Addr, u8)> {
    let (ip, len) = s.trim().split_once('/')?;
    let ip: Ipv4Addr = ip.parse().ok()?;
    let len: u8 = len.parse().ok()?;
    (len <= 32 && !ip.is_unspecified() && !ip.is_multicast() && !ip.is_broadcast()).then_some((ip, len))
}

/// Vérifie une demande (côté Core avant d'écrire, côté root avant d'appliquer).
pub fn check(w: &Wanted) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for i in &w.interfaces {
        if !valid_name(&i.name) {
            return Err(format!("not an interface name: {}", i.name));
        }
        if !seen.insert(&i.name) {
            return Err(format!("{} is set twice", i.name));
        }
        if i.mode == Mode::Static {
            if i.addresses.is_empty() {
                return Err(format!("{}: a fixed address is needed (10.0.0.2/24)", i.name));
            }
            for a in &i.addresses {
                parse_cidr(a).ok_or_else(|| format!("{}: not an address with its prefix: {a} (10.0.0.2/24)", i.name))?;
            }
        }
        if let Some(g) = &i.gateway {
            g.parse::<Ipv4Addr>().map_err(|_| format!("{}: not a gateway address: {g}", i.name))?;
        }
        for d in &i.dns {
            d.parse::<Ipv4Addr>().map_err(|_| format!("{}: not a DNS server address: {d}", i.name))?;
        }
        if let Some(m) = i.mtu {
            if !(576..=9216).contains(&m) {
                return Err(format!("{}: the MTU is between 576 and 9216", i.name));
            }
        }
    }
    Ok(())
}

/// Le fichier systemd-networkd d'une interface.
pub fn render(i: &IfaceIp) -> String {
    let mut s = format!("# Strata Core : réglé dans Réglages › Matériel › Réseau.\n[Match]\nName={}\n", i.name);
    if let Some(m) = i.mtu {
        s.push_str(&format!("\n[Link]\nMTUBytes={m}\n"));
    }
    s.push_str("\n[Network]\n");
    match i.mode {
        Mode::Dhcp => s.push_str("DHCP=ipv4\n"),
        Mode::Static => {
            s.push_str("DHCP=no\n");
            for a in &i.addresses {
                s.push_str(&format!("Address={}\n", a.trim()));
            }
            // Un port média sans câble garde son adresse (MTL et les flux la demandent).
            s.push_str("ConfigureWithoutCarrier=yes\n");
        }
    }
    if let Some(g) = &i.gateway {
        s.push_str(&format!("Gateway={g}\n"));
    }
    for d in &i.dns {
        s.push_str(&format!("DNS={d}\n"));
    }
    s.push_str("LinkLocalAddressing=no\nIPv6AcceptRA=no\n");
    s
}

fn file_of(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("05-strata-{name}.network"))
}

/// Nos fichiers déjà posés : interface → contenu.
fn ours(dir: &Path) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .filter_map(|e| {
                    let f = e.file_name().into_string().ok()?;
                    let name = f.strip_prefix("05-strata-")?.strip_suffix(".network")?.to_string();
                    Some((name, std::fs::read_to_string(e.path()).ok()?))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Pose les fichiers de `w` dans `dir` (et retire ceux des interfaces qui ne sont plus réglées) ;
/// rend les interfaces touchées.
fn write_files(dir: &Path, w: &Wanted) -> Result<Vec<String>, String> {
    let before = ours(dir);
    let mut touched: Vec<String> = Vec::new();
    for (name, text) in &before {
        if !w.interfaces.iter().any(|i| &i.name == name) {
            std::fs::remove_file(file_of(dir, name)).map_err(|e| e.to_string())?;
            touched.push(name.clone());
        } else if w.interfaces.iter().any(|i| &i.name == name && render(i) != *text) {
            touched.push(name.clone());
        }
    }
    for i in &w.interfaces {
        let text = render(i);
        if !before.iter().any(|(n, t)| n == &i.name && *t == text) {
            std::fs::write(file_of(dir, &i.name), &text).map_err(|e| format!("cannot write {}: {e}", file_of(dir, &i.name).display()))?;
            if !touched.contains(&i.name) {
                touched.push(i.name.clone());
            }
        }
    }
    Ok(touched)
}

// ---------- Côté root : `strata-core net-setup` / `net-revert` ----------

/// Le service `strata-net` (posé par l'installateur).
pub const UNIT: &str = "[Unit]\n\
Description=Strata: IP addresses of the network interfaces\n\
After=systemd-networkd.service\n\
\n\
[Service]\n\
Type=oneshot\n\
ExecStart=/usr/lib/strata/strata-media-setup net-setup\n";

fn save(o: &Outcome) {
    let _ = put(&outcome_path(), &serde_json::to_string_pretty(o).unwrap_or_default());
}

/// Un fichier de réglages écrit d'un coup (une coupure ne laisse jamais un fichier à moitié écrit).
fn put(path: &Path, text: &str) -> Result<(), String> {
    strata_core::files::write_atomic(path, text.as_bytes(), false).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// systemd-networkd relit ses fichiers et reprend ces interfaces.
fn reload(touched: &[String]) -> Result<(), String> {
    if touched.is_empty() {
        return Ok(());
    }
    if let Some(e) = super::clock::run_for("networkctl", &["reload"], Duration::from_secs(20)) {
        return Err(format!("networkctl reload: {e} (is the network run by systemd-networkd?)"));
    }
    for n in touched {
        // Une interface absente (retirée, passée en DPDK) n'empêche pas les autres.
        if let Some(e) = super::clock::run_for("networkctl", &["reconfigure", n], Duration::from_secs(20)) {
            tracing::warn!("networkctl reconfigure {n}: {e}");
        }
    }
    Ok(())
}

/// Applique `/etc/strata/network.json`.
pub fn setup() -> Result<(), String> {
    let r = (|| -> Result<Option<u64>, String> {
        let w = read_wanted();
        check(&w)?;
        let dir = networkd_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        // L'état d'avant, pour le retour arrière : nos fichiers et la demande d'avant.
        let prev = dir.join(".strata-prev");
        let _ = std::fs::remove_dir_all(&prev);
        std::fs::create_dir_all(&prev).map_err(|e| e.to_string())?;
        for (name, text) in ours(&dir) {
            std::fs::write(file_of(&prev, &name), text).map_err(|e| e.to_string())?;
        }
        let touched = write_files(&dir, &w)?;
        // Un retour arrière en attente : remplacé par celui de ce changement.
        let _ = super::clock::run_for("systemctl", &["stop", "strata-net-revert.timer"], Duration::from_secs(10));
        let _ = super::clock::run_for("systemctl", &["reset-failed", "strata-net-revert.service"], Duration::from_secs(10));
        reload(&touched)?;
        if w.guard && !touched.is_empty() {
            let secs = REVERT_SECS.to_string();
            let helper = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "/usr/lib/strata/strata-media-setup".into());
            if let Some(e) = super::clock::run_for(
                "systemd-run",
                &["--unit=strata-net-revert", "--collect", &format!("--on-active={secs}"), "--timer-property=AccuracySec=1s", &helper, "net-revert"],
                Duration::from_secs(10),
            ) {
                return Err(format!("cannot plan the way back: {e}"));
            }
            return Ok(Some(now_secs() + REVERT_SECS));
        }
        Ok(None)
    })();
    match &r {
        Ok(at) => save(&Outcome { ok: true, revert_at: *at, ..Default::default() }),
        Err(e) => save(&Outcome { ok: false, error: Some(e.clone()), ..Default::default() }),
    }
    r.map(|_| ())
}

/// Remet l'état d'avant le dernier changement (personne ne l'a confirmé).
pub fn revert() -> Result<(), String> {
    let dir = networkd_dir();
    let prev = dir.join(".strata-prev");
    let before: Vec<(String, String)> = ours(&prev);
    let now: Vec<(String, String)> = ours(&dir);
    let mut touched: Vec<String> = now.iter().map(|(n, _)| n.clone()).collect();
    for (n, _) in &now {
        let _ = std::fs::remove_file(file_of(&dir, n));
    }
    for (n, text) in &before {
        std::fs::write(file_of(&dir, n), text).map_err(|e| e.to_string())?;
        if !touched.contains(n) {
            touched.push(n.clone());
        }
    }
    // La demande aussi : la page montre ce qui est en place.
    if let Ok(text) = std::fs::read_to_string(prev_json()) {
        let _ = put(&etc(), &text);
    }
    let r = reload(&touched);
    save(&Outcome { ok: r.is_ok(), error: r.as_ref().err().cloned(), revert_at: None, reverted: true });
    tracing::warn!("network settings reverted ({})", touched.join(", "));
    r
}

// ---------- Côté Core ----------

/// Un changement attend d'être confirmé.
pub fn pending() -> Option<u64> {
    let o = read_outcome()?;
    let at = o.revert_at?;
    // Le minuteur est-il toujours là ? (confirmé ou passé : non)
    let active = std::process::Command::new("systemctl").args(["is-active", "--quiet", "strata-net-revert.timer"]).status().is_ok_and(|s| s.success());
    (active && at > now_secs()).then_some(at)
}

/// La dernière erreur d'application (aussi celle d'un changement appliqué après la réponse).
static LAST_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub fn last_error() -> Option<String> {
    LAST_ERROR.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Écrit la demande et la fait appliquer (bloquant).
pub fn apply(w: &Wanted) -> Result<Outcome, String> {
    let r = apply_inner(w);
    *LAST_ERROR.lock().unwrap_or_else(|e| e.into_inner()) = r.as_ref().err().cloned();
    r
}

fn apply_inner(w: &Wanted) -> Result<Outcome, String> {
    check(w)?;
    // L'état actuel, pour un retour arrière.
    let _ = put(&prev_json(), &serde_json::to_string_pretty(&read_wanted()).unwrap_or_default());
    put(&etc(), &serde_json::to_string_pretty(w).unwrap_or_default())?;
    if let Some(e) = super::clock::run_for("systemctl", &["restart", "strata-net"], Duration::from_secs(60)) {
        // Rien n'a été appliqué : la demande d'avant reste celle en place.
        if read_outcome().is_none_or(|o| o.ok) {
            if let Ok(prev) = std::fs::read_to_string(prev_json()) {
                let _ = put(&etc(), &prev);
            }
        }
        return Err(read_outcome().and_then(|o| o.error).unwrap_or(format!("strata-net: {e} (run the installer again and choose Repair)")));
    }
    match read_outcome() {
        Some(o) if o.ok => Ok(o),
        Some(o) => Err(o.error.unwrap_or("the network setup failed".into())),
        None => Err("strata-net gave no outcome".into()),
    }
}

/// Garde le changement en attente.
pub fn confirm() -> Result<(), String> {
    match super::clock::run_for("systemctl", &["stop", "strata-net-revert.timer"], Duration::from_secs(10)) {
        None => Ok(()),
        Some(e) => Err(format!("cannot keep the change: {e}")),
    }
}

/// Revient tout de suite à l'état d'avant.
pub fn revert_now() -> Result<(), String> {
    match super::clock::run_for("systemctl", &["start", "strata-net-revert.service"], Duration::from_secs(30)) {
        None => Ok(()),
        Some(e) => Err(format!("cannot go back: {e}")),
    }
}

/// L'interface qui porte la route par défaut.
pub fn default_route_iface() -> Option<String> {
    std::fs::read_to_string("/proc/net/route").ok()?.lines().skip(1).find_map(|l| {
        let mut f = l.split_whitespace();
        let iface = f.next()?;
        (f.next()? == "00000000").then(|| iface.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(name: &str, a: &str) -> IfaceIp {
        IfaceIp {
            name: name.into(),
            mode: Mode::Static,
            addresses: vec![a.into()],
            gateway: Some("10.0.0.1".into()),
            dns: vec!["10.0.0.53".into()],
            mtu: Some(9000),
        }
    }

    #[test]
    fn checks_and_renders() {
        let ok = Wanted { interfaces: vec![iface("ens2f0", "10.0.0.2/24")], guard: false };
        assert!(check(&ok).is_ok());
        let text = render(&ok.interfaces[0]);
        assert!(
            text.contains("Name=ens2f0")
                && text.contains("Address=10.0.0.2/24")
                && text.contains("Gateway=10.0.0.1")
                && text.contains("MTUBytes=9000")
                && text.contains("DHCP=no")
        );
        for bad in [iface("ens2f0;rm", "10.0.0.2/24"), iface("ens2f0", "10.0.0.2"), iface("ens2f0", "239.0.0.1/24"), iface("ens2f0", "10.0.0.2/33")] {
            assert!(check(&Wanted { interfaces: vec![bad], guard: false }).is_err());
        }
        let mut m = iface("eno1", "10.0.0.2/24");
        m.mtu = Some(100);
        assert!(check(&Wanted { interfaces: vec![m], guard: false }).is_err());
        let dhcp = IfaceIp { name: "eno1".into(), mode: Mode::Dhcp, ..Default::default() };
        assert!(check(&Wanted { interfaces: vec![dhcp.clone()], guard: false }).is_ok());
        assert!(render(&dhcp).contains("DHCP=ipv4"));
    }

    #[test]
    fn writes_only_what_changed() {
        let dir = std::env::temp_dir().join(format!("strata-ip-{}", strata_core::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let w = Wanted { interfaces: vec![iface("ens2f0", "10.0.0.2/24"), iface("ens2f1", "10.0.1.2/24")], guard: false };
        assert_eq!(write_files(&dir, &w).unwrap(), vec!["ens2f0".to_string(), "ens2f1".to_string()]);
        assert!(write_files(&dir, &w).unwrap().is_empty(), "nothing changed");
        // ens2f1 rendue au système, ens2f0 changée.
        let w2 = Wanted { interfaces: vec![iface("ens2f0", "10.0.0.3/24")], guard: false };
        let mut t = write_files(&dir, &w2).unwrap();
        t.sort();
        assert_eq!(t, vec!["ens2f0".to_string(), "ens2f1".to_string()]);
        assert!(!file_of(&dir, "ens2f1").exists());
        assert!(std::fs::read_to_string(file_of(&dir, "ens2f0")).unwrap().contains("10.0.0.3/24"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
