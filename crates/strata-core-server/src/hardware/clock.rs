//! L'horloge du site (Réglages › Matériel › Horloge) : la cadence maison, d'où le serveur prend
//! l'heure, le timecode maison, et un bilan.
//!
//! - **L'heure** : un seul service la règle, chrony. Avec le PTP, chrony suit l'horloge de la carte
//!   réseau PTP (tenue par ptp4l, en TAI) et garde les serveurs NTP en secours ; sans PTP, il suit
//!   les serveurs NTP. phc2sys ne doit pas régler l'horloge en même temps (signalé dans le bilan).
//! - **Les fichiers** : le Core écrit `/etc/chrony/conf.d/strata.conf`, et pour le PTP
//!   `/etc/strata/ptp4l.conf` et le service `strata-ptp4l`, puis relance ce qu'il faut.
//!   Sans les droits (l'installateur les donnera), il le dit, et les fichiers restent lisibles dans
//!   l'interface et dans `<données>/clock/`. `STRATA_SYSTEM_ROOT` : écrire ailleurs qu'à la racine
//!   (essais), sans rien relancer.
//! - **Le timecode maison** : `strata_core::timecode`, à partir de l'heure du serveur.
//! - **Le bilan** : l'heure suit-elle le PTP, les REF des cartes sont-elles verrouillées et à la
//!   cadence maison, le timecode des entrées SDI est-il celui de la maison.
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{
    net::IpAddr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use strata_core::timecode::{house_timecode, HouseClock, HouseRate, Timecode};

use super::{capture, decklink, net, sync::SyncStatus, HardwareConfig};

/// D'où le serveur prend l'heure.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TimeSource {
    /// Le PTP (l'horloge de la carte réseau PTP), les serveurs NTP en secours.
    #[default]
    Ptp,
    /// Les serveurs NTP seulement.
    Ntp,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct ClockSettings {
    pub source: TimeSource,
    /// Les serveurs NTP (le leader en sert souvent un), dans l'ordre.
    pub ntp_servers: Vec<String>,
    /// La cadence et le timecode maison.
    pub house: HouseClock,
}

fn valid_host(h: &str) -> bool {
    h.parse::<IpAddr>().is_ok()
        || (h.len() <= 253
            && h.split('.').all(|l| {
                !l.is_empty()
                    && l.len() <= 63
                    && !l.starts_with('-')
                    && !l.ends_with('-')
                    && l.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            }))
}

impl ClockSettings {
    pub fn check(&mut self) -> Result<(), String> {
        let mut servers: Vec<String> = Vec::new();
        for s in &self.ntp_servers {
            let s = s.trim().to_ascii_lowercase();
            if s.is_empty() || servers.contains(&s) {
                continue;
            }
            if !valid_host(&s) {
                return Err(format!("not an NTP server: {s}"));
            }
            servers.push(s);
        }
        if servers.len() > 8 {
            return Err("at most 8 NTP servers".into());
        }
        self.ntp_servers = servers;
        let jam = self.house.daily_jam.trim();
        let ok = jam
            .split_once(':')
            .is_some_and(|(h, m)| h.len() == 2 && m.len() == 2 && h.parse::<u8>().is_ok_and(|h| h < 24) && m.parse::<u8>().is_ok_and(|m| m < 60));
        if !ok {
            return Err("the daily jam is written HH:MM".into());
        }
        self.house.daily_jam = jam.to_string();
        if self.house.fixed_minutes.abs() > 24 * 60 {
            return Err("the timecode offset is out of range".into());
        }
        Ok(())
    }
}

// ---------- Les fichiers du système ----------

/// Ce que le Core écrit pour que le système tienne l'heure comme réglé.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SystemFiles {
    /// `/etc/chrony/conf.d/strata.conf`.
    pub chrony: String,
    /// `/etc/strata/ptp4l.conf` (None : pas de PTP).
    pub ptp4l: Option<String>,
    /// `/etc/systemd/system/strata-ptp4l.service` (toujours le même).
    pub ptp4l_unit: Option<String>,
    /// La carte PTP et son horloge matérielle.
    pub ptp_interface: Option<String>,
    pub phc: Option<String>,
}

const CHRONY: &str = "etc/chrony/conf.d/strata.conf";
const PTP4L: &str = "etc/strata/ptp4l.conf";
const UNIT: &str = "etc/systemd/system/strata-ptp4l.service";

/// Le service ptp4l de Strata : toujours là (posé par l'installateur), il ne démarre que si ses
/// réglages existent ; le Core le relance ou l'arrête (droits donnés par l'installateur, polkit).
pub const PTP4L_UNIT: &str = "[Unit]\n\
Description=Strata Core : PTP (ptp4l, ST 2059-2)\n\
After=network-online.target\n\
Wants=network-online.target\n\
ConditionPathExists=/etc/strata/ptp4l.conf\n\
\n\
[Service]\n\
ExecStart=/usr/sbin/ptp4l -f /etc/strata/ptp4l.conf\n\
Restart=always\n\
RestartSec=2\n\
\n\
[Install]\n\
WantedBy=multi-user.target\n";

pub fn files(cfg: &HardwareConfig) -> SystemFiles {
    let c = &cfg.clock;
    let iface = Some(cfg.network.ptp.trim()).filter(|i| !i.is_empty() && c.source == TimeSource::Ptp).map(str::to_string);
    let phc = iface.as_deref().and_then(net::phc_of);
    let mut chrony = String::from(
        "# Strata Core : l'heure du serveur (Réglages › Matériel › Horloge).\n# Écrit par le Core : modifiez les réglages plutôt que ce fichier.\n",
    );
    match (&iface, &phc) {
        (Some(i), Some(p)) => {
            chrony.push_str(&format!(
                "\n# PTP d'abord : l'horloge de {i}, tenue par ptp4l (domaine {}), est en TAI ; − {} s pour l'UTC.\nrefclock PHC /dev/{p} poll 0 dpoll -2 offset -{} prefer refid PTP\n",
                cfg.ptp.domain, cfg.ptp.tai_offset, cfg.ptp.tai_offset
            ));
        }
        (Some(i), None) => chrony.push_str(&format!("\n# PTP : {i} n'a pas d'horloge matérielle, le PTP ne peut pas tenir l'heure.\n")),
        (None, _) if c.source == TimeSource::Ptp => chrony.push_str("\n# PTP : aucune carte réseau n'a le rôle PTP (Réglages › Matériel › Réseau).\n"),
        _ => {}
    }
    if c.ntp_servers.is_empty() {
        chrony.push_str("\n# NTP : aucun serveur réglé ; ceux du système restent utilisés.\n");
    } else {
        chrony.push_str(if c.source == TimeSource::Ptp { "\n# NTP en secours.\n" } else { "\n# NTP.\n" });
        for s in &c.ntp_servers {
            chrony.push_str(&format!("server {s} iburst\n"));
        }
    }
    let ptp4l = match (&iface, &phc) {
        (Some(i), Some(_)) => Some(format!(
            "# Strata Core : PTP (SMPTE ST 2059-2), esclave seulement. Écrit par le Core.\n\
             [global]\n\
             domainNumber            {}\n\
             slaveOnly               1\n\
             time_stamping           hardware\n\
             network_transport       UDPv4\n\
             delay_mechanism         E2E\n\
             logAnnounceInterval     -2\n\
             logSyncInterval         -3\n\
             logMinDelayReqInterval  -3\n\
             announceReceiptTimeout  3\n\
             dscp_event              46\n\
             dscp_general            46\n\
             tx_timestamp_timeout    50\n\
             \n\
             [{i}]\n",
            cfg.ptp.domain
        )),
        _ => None,
    };
    let unit = Some(PTP4L_UNIT.to_string());
    SystemFiles { chrony, ptp4l, ptp4l_unit: unit, ptp_interface: iface, phc }
}

/// Une étape de la mise en place.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    /// `chrony`, `ptp4l`, `unit`, `restartChrony`, `startPtp4l`, `stopPtp4l`.
    pub what: &'static str,
    pub ok: bool,
    pub message: Option<String>,
}

fn system_root() -> (PathBuf, bool) {
    match std::env::var_os("STRATA_SYSTEM_ROOT") {
        Some(r) if !r.is_empty() => (PathBuf::from(r), false),
        _ => (PathBuf::from("/"), true),
    }
}

/// Lance une commande (15 s au plus) : None si elle réussit, sinon pourquoi.
pub(crate) fn run(program: &str, args: &[&str]) -> Option<String> {
    let child = Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return Some(format!("{program}: {e}")),
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => return None,
            Ok(Some(_)) => {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut e, &mut err);
                }
                return Some(err.trim().lines().last().unwrap_or("failed").to_string());
            }
            Ok(None) if start.elapsed() < Duration::from_secs(15) => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                return Some(format!("{program} did not finish"));
            }
        }
    }
}

/// Écrit un fichier s'il change : Ok(vrai) s'il a changé.
fn put(path: &Path, content: Option<&str>) -> Result<bool, String> {
    let old = std::fs::read_to_string(path).ok();
    match content {
        Some(c) if old.as_deref() == Some(c) => Ok(false),
        Some(c) => {
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            }
            std::fs::write(path, c).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            Ok(true)
        }
        None if old.is_some() => std::fs::remove_file(path).map(|_| true).map_err(|e| format!("cannot remove {}: {e}", path.display())),
        None => Ok(false),
    }
}

/// Met en place les fichiers et relance ce qui a changé (`force` : relancer quand même).
pub fn apply(data_dir: &Path, f: &SystemFiles, force: bool) -> Vec<Step> {
    // Une copie dans les données du Core, toujours lisible.
    let keep = data_dir.join("clock");
    let _ = put(&keep.join("chrony-strata.conf"), Some(&f.chrony));
    let _ = put(&keep.join("strata-ptp4l.conf"), f.ptp4l.as_deref());

    let (root, live) = system_root();
    let mut steps = Vec::new();
    let mut write = |what: &'static str, rel: &str, content: Option<&str>| -> Option<bool> {
        match put(&root.join(rel), content) {
            Ok(changed) => {
                steps.push(Step { what, ok: true, message: None });
                Some(changed)
            }
            Err(e) => {
                steps.push(Step { what, ok: false, message: Some(e) });
                None
            }
        }
    };
    let chrony = write("chrony", CHRONY, Some(&f.chrony));
    let ptp = write("ptp4l", PTP4L, f.ptp4l.as_deref());
    let unit = write("unit", UNIT, f.ptp4l_unit.as_deref());
    if !live {
        return steps;
    }
    let step = |what, r: Option<String>| Step { what, ok: r.is_none(), message: r };
    if unit == Some(true) {
        let _ = run("systemctl", &["daemon-reload"]);
        let _ = run("systemctl", &["enable", "strata-ptp4l"]);
    }
    if f.ptp4l.is_some() {
        if ptp.is_some() && (force || ptp == Some(true)) {
            steps.push(step("startPtp4l", run("systemctl", &["restart", "strata-ptp4l"])));
        }
    } else if ptp == Some(true) {
        // Ses réglages viennent d'être retirés.
        steps.push(step("stopPtp4l", run("systemctl", &["stop", "strata-ptp4l"])));
    }
    if chrony.is_some() && (force || chrony == Some(true)) {
        steps.push(step("restartChrony", run("systemctl", &["restart", "chrony"])));
    }
    steps
}

// ---------- Le timecode maison, maintenant ----------

pub fn now_ns() -> i128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as i128)
}

/// Le timecode maison à cet instant, pour l'affichage (qui le fait avancer de lui-même).
pub fn house_now(cfg: &HardwareConfig, tz: &str) -> Value {
    let now = now_ns();
    let h = &cfg.clock.house;
    let tc = house_timecode(now, h, tz, cfg.ptp.tai_offset);
    let (num, den) = h.rate.ratio();
    json!({
        "timecode": tc.to_string(),
        "frame": tc.frame_of_day(h.rate.nominal()),
        "unixMs": (now / 1_000_000) as u64,
        "fps": num as f64 / den as f64,
        "nominal": h.rate.nominal(),
        "dropFrame": tc.drop_frame,
        "utcOffset": strata_core::timecode::utc_offset(h, tz, (now / 1_000_000_000) as i64),
    })
}

// ---------- Le bilan ----------

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    Ok,
    Info,
    Warn,
    Bad,
}

/// Un point du bilan : `code` et ses `params` sont mis en mots par l'interface.
#[derive(Serialize, Clone, Debug)]
pub struct Check {
    pub code: &'static str,
    pub level: Level,
    pub params: Map<String, Value>,
}

fn check(code: &'static str, level: Level, params: Value) -> Check {
    Check { code, level, params: params.as_object().cloned().unwrap_or_default() }
}

/// La cadence d'un format (« 1080i50 », « 525i59.94 », « 720p25 »).
fn rate_of_format(f: &str) -> Option<HouseRate> {
    // La cadence suit la dernière lettre : 1080i50, 720p25, 1080psf25.
    let i = f.rfind(['i', 'p', 'f'])?;
    let v: f64 = f[i + 1..].parse().ok()?;
    HouseRate::of_fps(v)
}

fn rate_name(r: HouseRate) -> &'static str {
    match r {
        HouseRate::R25 => "25",
        HouseRate::R2997 => "29.97",
        HouseRate::R30 => "30",
        HouseRate::R24 => "24",
        HouseRate::R2398 => "23.98",
    }
}

/// L'écart entre le timecode d'une entrée et le timecode maison, en images (None : pas de timecode).
fn input_gap(share: &Path, cfg: &HardwareConfig, tz: &str) -> Option<i64> {
    let mut r = strata_share::Reader::observe(share).ok()?;
    let mut tc = None;
    let f = r.latest_with(|f, _| tc = f.info.timecode)?;
    let t = tc?;
    let nominal = cfg.clock.house.rate.nominal();
    let theirs = Timecode { hours: t.hours, minutes: t.minutes, seconds: t.seconds, frames: t.frames, drop_frame: t.drop_frame };
    let ours = house_timecode(f.captured_ns as i128, &cfg.clock.house, tz, cfg.ptp.tai_offset);
    Some(Timecode { drop_frame: ours.drop_frame, ..theirs }.frames_from(&ours, nominal))
}

pub fn checks(cfg: &HardwareConfig, st: &SyncStatus, cards: &[decklink::DetectedCard], inputs: &[capture::Status], tz: &str) -> Vec<Check> {
    use Level::*;
    let mut out = Vec::new();
    let house = cfg.clock.house.rate;
    let ptp_iface = cfg.network.ptp.trim();
    let from_ptp = st.ntp.source.as_deref() == Some("PTP");

    // D'où vient l'heure.
    match cfg.clock.source {
        TimeSource::Ptp => {
            if ptp_iface.is_empty() {
                out.push(check("ptpNoInterface", Bad, json!({})));
            } else if net::phc_of(ptp_iface).is_none() {
                out.push(check("ptpNoPhc", Bad, json!({"interface": ptp_iface})));
            }
            if st.ptp.locked && from_ptp {
                out.push(check("timeFromPtp", Ok, json!({"grandmaster": st.ptp.grandmaster, "offsetNs": st.ptp.offset_ns})));
            } else if st.ptp.locked {
                out.push(check("ptpNotFollowed", Warn, json!({})));
            } else if st.ntp.synced == Some(true) {
                out.push(check("ptpFallbackNtp", Warn, json!({"server": st.ntp.source, "running": st.ptp.running})));
            } else {
                out.push(check("noTime", Bad, json!({})));
            }
        }
        TimeSource::Ntp => {
            if st.ntp.synced == Some(true) {
                out.push(check("timeFromNtp", Ok, json!({"server": st.ntp.source, "offsetMs": st.ntp.offset_ms})));
            } else {
                out.push(check("noTime", Bad, json!({})));
            }
        }
    }
    if st.ptp.sets_clock && st.ntp.synced == Some(true) {
        out.push(check("twoClockSetters", Warn, json!({})));
    }

    // Les références des cartes DeckLink.
    for d in cards.iter().filter(|d| d.has_reference) {
        let c = cfg.cards.iter().find(|c| c.id == d.id);
        let name = c.map(|c| c.name.as_str()).filter(|n| !n.is_empty()).unwrap_or(&d.model_name);
        let genlock = c.is_some_and(|c| c.genlock);
        match (d.reference_locked, &d.reference_format) {
            (Some(true), fmt) => {
                let rate = fmt.as_deref().and_then(rate_of_format);
                if rate.is_some_and(|r| r != house) {
                    out.push(check("refWrongRate", Bad, json!({"card": name, "format": fmt, "house": rate_name(house)})));
                } else {
                    out.push(check(
                        if genlock { "refLocked" } else { "refLockedNoGenlock" },
                        if genlock { Ok } else { Info },
                        json!({"card": name, "format": fmt}),
                    ));
                }
            }
            (Some(false), _) => {
                out.push(check(if genlock { "refMissing" } else { "refMissingNoGenlock" }, if genlock { Bad } else { Info }, json!({"card": name})))
            }
            (None, _) => {}
        }
    }

    // Les entrées SDI : leur cadence, leur timecode.
    for i in inputs {
        let card = cfg.cards.iter().find(|c| c.id == i.card);
        let wire = card.and_then(|c| c.connectors.get(i.connector as usize - 1)).map(|k| k.name.as_str()).filter(|n| !n.is_empty());
        let name = wire.map_or_else(|| format!("SDI {}", i.connector), |w| format!("SDI {} · {w}", i.connector));
        if i.signal != Some(true) {
            continue;
        }
        if let Some(r) = i.format.as_deref().and_then(rate_of_format) {
            if r != house {
                out.push(check("inputWrongRate", Warn, json!({"input": name, "format": i.format, "house": rate_name(house)})));
                continue;
            }
        }
        match input_gap(&i.share, cfg, tz) {
            Some(0) => out.push(check("inputTcMatches", Ok, json!({"input": name}))),
            Some(g) => out.push(check("inputTcOff", Warn, json!({"input": name, "frames": g}))),
            None => out.push(check("inputNoTc", Info, json!({"input": name}))),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_checked() {
        let mut c = ClockSettings { ntp_servers: vec![" 10.0.0.1 ".into(), "NTP.Regie.fr".into(), "10.0.0.1".into(), "".into()], ..ClockSettings::default() };
        c.check().unwrap();
        assert_eq!(c.ntp_servers, ["10.0.0.1", "ntp.regie.fr"]);
        c.ntp_servers.push("pas un nom".into());
        assert_eq!(c.check().unwrap_err(), "not an NTP server: pas un nom");
        let mut c = ClockSettings::default();
        c.house.daily_jam = "6:00".into();
        assert_eq!(c.check().unwrap_err(), "the daily jam is written HH:MM");
    }

    #[test]
    fn formats_have_a_rate() {
        assert_eq!(rate_of_format("625i50"), Some(HouseRate::R25));
        assert_eq!(rate_of_format("1080i59.94"), Some(HouseRate::R2997));
        assert_eq!(rate_of_format("720p25"), Some(HouseRate::R25));
        assert_eq!(rate_of_format("1080p23.98"), Some(HouseRate::R2398));
        assert_eq!(rate_of_format("1080psf25"), Some(HouseRate::R25));
        assert_eq!(rate_of_format("pal "), None);
    }
}
