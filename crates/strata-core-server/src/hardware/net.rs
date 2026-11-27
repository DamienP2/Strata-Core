//! Les cartes réseau du serveur (Réglages › Matériel › Réseau) : leurs adresses, leur lien, leur
//! débit, leur famille. Repris de CG (`cg-server/src/hardware.rs`), Linux seulement.
use serde::Serialize;
use std::net::Ipv4Addr;

/// Une carte réseau et son lien.
#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Nic {
    pub name: String,
    /// `10.10.0.21/24`.
    pub ipv4: Vec<String>,
    pub mac: Option<String>,
    pub mtu: Option<u32>,
    /// Débit du lien, Mb/s.
    pub speed: Option<u32>,
    /// Le lien est monté (None : inconnu).
    pub up: Option<bool>,
    /// `ethernet`, `wifi`, `virtual`.
    pub kind: Option<&'static str>,
    /// La famille de la carte, d'après son pilote (« Mellanox ConnectX »).
    pub model: Option<String>,
    /// La carte PCI (`0000:3b:00`) : les ports d'une même carte la partagent.
    pub pci: Option<String>,
    /// Le numéro du port sur sa carte (0 = le premier : la fonction PCI).
    pub port: Option<u32>,
    /// Sur la carte mère (`eno1`), pas sur une carte ajoutée.
    pub onboard: bool,
    /// `sfp` (cage pour module ou câble) ou `rj45`, d'après la famille de la carte.
    pub connector: Option<&'static str>,
    /// Son horloge matérielle PTP (`ptp0`) : horodatage matériel possible.
    pub phc: Option<String>,
}

/// Les familles de cartes à cages SFP (10, 25, 100 Gb/s).
fn sfp_driver(driver: &str) -> bool {
    matches!(driver, "mlx5_core" | "mlx4_en" | "mlx4_core" | "ice" | "i40e" | "ixgbe" | "bnxt_en" | "sfc")
}

/// `0000:3b:00.1` → (`0000:3b:00`, 1).
fn pci_slot(address: &str) -> Option<(String, u32)> {
    let (slot, func) = address.rsplit_once('.')?;
    if slot.matches(':').count() != 2 {
        return None;
    }
    Some((slot.to_string(), func.parse().ok()?))
}

/// La famille d'une carte d'après son pilote Linux.
fn driver_model(driver: &str) -> Option<&'static str> {
    Some(match driver {
        "mlx5_core" => "Mellanox ConnectX",
        "mlx4_en" | "mlx4_core" => "Mellanox ConnectX-3",
        "ice" => "Intel E810",
        "i40e" => "Intel X710",
        "ixgbe" => "Intel X520",
        "igb" => "Intel I350",
        "igc" => "Intel I225",
        "e1000e" | "e1000" => "Intel Gigabit",
        "bnxt_en" => "Broadcom NetXtreme-E",
        "tg3" => "Broadcom NetXtreme",
        "r8169" | "r8125" => "Realtek",
        "atlantic" => "Aquantia AQtion",
        "sfc" => "Solarflare",
        "virtio_net" => "virtio",
        _ => return None,
    })
}

/// L'adresse IPv4 d'une interface, par son nom.
pub fn interface_ip(name: &str) -> Option<Ipv4Addr> {
    if name.is_empty() {
        return None;
    }
    // Une interface peut avoir une adresse IPv6 listée avant son IPv4.
    if_addrs::get_if_addrs().ok()?.into_iter().filter(|i| i.name == name).find_map(|i| match i.ip() {
        std::net::IpAddr::V4(ip) => Some(ip),
        _ => None,
    })
}

/// Des cartes réseau simulées (`STRATA_FAKE_NICS=connectx,i350`), pour travailler sans elles.
fn fake_nics() -> Option<Vec<Nic>> {
    let v = std::env::var("STRATA_FAKE_NICS").ok().filter(|v| !v.trim().is_empty())?;
    let mut out = vec![Nic {
        name: "eno1".into(),
        ipv4: vec!["10.0.0.20/24".into()],
        mac: Some("3c:ec:ef:10:00:01".into()),
        mtu: Some(1500),
        speed: Some(1000),
        up: Some(true),
        kind: Some("ethernet"),
        model: Some("Intel I350".into()),
        pci: Some("0000:01:00".into()),
        port: Some(0),
        onboard: true,
        connector: Some("rj45"),
        phc: Some("ptp0".into()),
    }];
    for (slot, m) in v.split(',').map(str::trim).enumerate() {
        let (model, ports, speed, sfp, mtu): (&str, u32, u32, bool, u32) = match m {
            "connectx" => ("Mellanox ConnectX", 2, 25000, true, 9000),
            "e810" => ("Intel E810", 2, 100000, true, 9000),
            "i350" => ("Intel I350", 4, 1000, false, 1500),
            _ => continue,
        };
        let bus = 0x3b + slot as u32 * 0x20;
        for f in 0..ports {
            out.push(Nic {
                name: format!("ens{}f{f}", slot + 2),
                ipv4: if f < 2 { vec![format!("192.168.{}.20/24", 110 + 10 * f + 100 * slot as u32)] } else { vec![] },
                mac: Some(format!("b8:ce:f6:{:02x}:00:{f:02x}", bus)),
                mtu: Some(mtu),
                speed: (f < 2).then_some(speed),
                up: Some(f < 2),
                kind: Some("ethernet"),
                model: Some(model.into()),
                pci: Some(format!("0000:{bus:02x}:00")),
                port: Some(f),
                onboard: false,
                connector: Some(if sfp { "sfp" } else { "rj45" }),
                phc: Some(format!("ptp{}", out.len())),
            });
        }
    }
    Some(out)
}

/// L'horloge PTP matérielle d'une interface (`ptp0`), d'après /sys.
pub fn phc_of(name: &str) -> Option<String> {
    if let Some(n) = fake_nics() {
        return n.into_iter().find(|n| n.name == name)?.phc;
    }
    let dir = std::fs::read_dir(std::path::Path::new("/sys/class/net").join(name).join("device/ptp")).ok()?;
    let mut v: Vec<String> = dir.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with("ptp")).collect();
    v.sort();
    v.into_iter().next()
}

/// Les cartes réseau, sans la boucle locale ; celles qui ont une adresse d'abord.
pub fn nics() -> Vec<Nic> {
    if let Some(fake) = fake_nics() {
        return fake;
    }
    let addrs = if_addrs::get_if_addrs().unwrap_or_default();
    let mut names: Vec<String> = Vec::new();
    if let Ok(dir) = std::fs::read_dir("/sys/class/net") {
        names.extend(dir.flatten().map(|e| e.file_name().to_string_lossy().into_owned()));
    }
    for a in &addrs {
        if !names.contains(&a.name) {
            names.push(a.name.clone());
        }
    }
    let mut out: Vec<Nic> = names
        .into_iter()
        .filter(|n| !addrs.iter().any(|a| a.name == *n && a.is_loopback()) && n != "lo")
        .map(|name| {
            let ipv4 = addrs
                .iter()
                .filter(|a| a.name == name)
                .filter_map(|a| match &a.addr {
                    if_addrs::IfAddr::V4(v) => Some(format!("{}/{}", v.ip, u32::from(v.netmask).count_ones())),
                    _ => None,
                })
                .collect();
            let mut n = Nic { name, ipv4, ..Nic::default() };
            let dir = std::path::Path::new("/sys/class/net").join(&n.name);
            let read = |f: &str| std::fs::read_to_string(dir.join(f)).ok().map(|s| s.trim().to_string());
            n.mac = read("address").filter(|m| !m.is_empty() && m != "00:00:00:00:00:00");
            n.mtu = read("mtu").and_then(|v| v.parse().ok());
            n.up = read("operstate").map(|s| s == "up" || (s == "unknown" && read("carrier").as_deref() == Some("1")));
            // -1 (ou illisible) sans lien.
            n.speed = read("speed").and_then(|v| v.parse::<i64>().ok()).filter(|v| *v > 0 && n.up == Some(true)).map(|v| v as u32);
            let driver = std::fs::read_link(dir.join("device/driver")).ok().and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()));
            n.kind = Some(if dir.join("wireless").exists() || dir.join("phy80211").exists() {
                "wifi"
            } else if dir.join("device").exists() {
                "ethernet"
            } else {
                "virtual"
            });
            n.model = driver.as_deref().and_then(driver_model).map(str::to_string);
            let address = std::fs::read_link(dir.join("device")).ok().and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()));
            if let Some((slot, func)) = address.as_deref().and_then(pci_slot) {
                n.pci = Some(slot);
                n.port = Some(func);
            }
            n.onboard = n.name.starts_with("eno");
            n.phc = phc_of(&n.name);
            if n.kind == Some("ethernet") {
                n.connector = Some(if driver.as_deref().is_some_and(sfp_driver) { "sfp" } else { "rj45" });
            }
            n
        })
        .collect();
    out.sort_by(|a, b| (a.ipv4.is_empty(), &a.name).cmp(&(b.ipv4.is_empty(), &b.name)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pci_slots() {
        assert_eq!(pci_slot("0000:3b:00.1"), Some(("0000:3b:00".into(), 1)));
        assert_eq!(pci_slot("0000:00:1f.6"), Some(("0000:00:1f".into(), 6)));
        assert_eq!(pci_slot("virtio0"), None);
        assert!(sfp_driver("mlx5_core") && !sfp_driver("igb"));
    }
}
