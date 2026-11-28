//! Où l'installateur pose les choses. Avec `--root`, tout va sous ce dossier (essai à blanc).
use std::path::{Path, PathBuf};

/// Le programme du Core, et son dossier (au compte du service : le Core se met à jour lui-même).
pub const BIN_DIR: &str = "/opt/strata/bin";
pub const BIN: &str = "/opt/strata/bin/strata-core";
/// Les bibliothèques livrées avec le Core (MXL).
pub const LIB_DIR: &str = "/opt/strata/lib";
/// La commande `strata-core` pour les administrateurs.
pub const LINK: &str = "/usr/local/bin/strata-core";
pub const DATA: &str = "/var/lib/strata-core";
/// Le compte du service.
pub const USER: &str = "strata";
pub const UNIT: &str = "/etc/systemd/system/strata-core.service";
pub const PTP_UNIT: &str = "/etc/systemd/system/strata-ptp4l.service";
/// Une copie du Core à root, que seul `strata-media` (root) lance : root n'exécute jamais un
/// binaire que le compte du service peut remplacer.
pub const HELPER_DIR: &str = "/usr/lib/strata";
pub const MEDIA_HELPER: &str = "/usr/lib/strata/strata-media-setup";
/// Hugepages et ports DPDK pour le ST 2110 (en root, relancé par le Core).
pub const MEDIA_UNIT: &str = "/etc/systemd/system/strata-media.service";
/// Les adresses IP des interfaces (en root, relancé par le Core).
pub const NET_UNIT: &str = "/etc/systemd/system/strata-net.service";
/// Le compte du service relance chrony et ptp4l (Matériel › Horloge).
pub const POLKIT: &str = "/etc/polkit-1/rules.d/50-strata-core.rules";
/// Les réglages que le Core écrit pour le système.
pub const ETC: &str = "/etc/strata";
pub const CHRONY: &str = "/etc/chrony/conf.d/strata.conf";
pub const SYSCTL: &str = "/etc/sysctl.d/60-strata.conf";
pub const LOG: &str = "/var/log/strata-install.log";
/// Le dépôt des releases du Core (et de l'installateur).
pub const REPO: &str = "DamienP2/Strata-Core";
/// Le port du cockpit.
pub const PORT: u16 = 80;

/// Un chemin du système, sous `root` (« / » : le vrai).
pub fn under(root: &Path, p: &str) -> PathBuf {
    if root == Path::new("/") {
        PathBuf::from(p)
    } else {
        root.join(p.trim_start_matches('/'))
    }
}
