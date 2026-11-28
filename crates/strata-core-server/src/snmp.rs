//! SNMP (Réglages › Santé › SNMP) : l'état du serveur et des applis pour les outils de supervision
//! de la régie. Un agent SNMP v2c en lecture seule (GET, GETNEXT, GETBULK) et des traps envoyés
//! avec les alertes. Le codage BER est repris de Strata Network.
//!
//! - `system` (MIB-2) : description, nom, temps de marche, contact, lieu.
//! - `STRATA-CORE-MIB` (sous [`ROOT`]) : le Core (version, nom, problèmes, processeurs, mémoire,
//!   disque, heure), les applis, les entrées SDI, les problèmes en cours ; la MIB se télécharge
//!   dans l'interface.
use std::{
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
};

use axum::{extract::State, http::header, response::IntoResponse, routing::get, Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::{net::UdpSocket, task::JoinHandle};

use crate::{
    api::{bad, require, ApiResult},
    auth::CurrentUser,
    sso::MASK,
    state::{AppState, Shared},
};

/// La racine de la MIB Strata : le numéro d'entreprise réservé à la documentation (RFC 5612),
/// à remplacer par celui de l'entreprise si elle en a un.
pub const ROOT: &[u32] = &[1, 3, 6, 1, 4, 1, 32473, 1, 1];

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct SnmpSettings {
    pub enabled: bool,
    pub port: u16,
    /// La communauté (lecture seule).
    pub community: String,
    /// Les machines qui peuvent interroger (adresses ou réseaux `10.0.0.0/24`) ; vide : toutes.
    pub allowed: Vec<String>,
    /// Où envoyer les traps (`10.0.0.9` ou `10.0.0.9:162`).
    pub traps: Vec<String>,
    pub contact: String,
    pub location: String,
}

impl Default for SnmpSettings {
    fn default() -> Self {
        SnmpSettings {
            enabled: false,
            port: 161,
            community: String::new(),
            allowed: Vec::new(),
            traps: Vec::new(),
            contact: String::new(),
            location: String::new(),
        }
    }
}

/// Une adresse ou un réseau : (adresse, longueur du préfixe).
pub(crate) fn net_of(s: &str) -> Option<(IpAddr, u8)> {
    let (ip, len) = s.split_once('/').map_or((s, None), |(a, b)| (a, Some(b)));
    let ip: IpAddr = ip.trim().parse().ok()?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    let len = match len {
        Some(l) => l.trim().parse().ok().filter(|l| *l <= max)?,
        None => max,
    };
    Some((ip, len))
}

pub(crate) fn in_net(ip: IpAddr, (net, len): (IpAddr, u8)) -> bool {
    match (ip, net) {
        (IpAddr::V4(a), IpAddr::V4(b)) => len == 0 || (u32::from(a) >> (32 - len as u32)) == (u32::from(b) >> (32 - len as u32)),
        (IpAddr::V6(a), IpAddr::V6(b)) => len == 0 || (u128::from(a) >> (128 - len as u32)) == (u128::from(b) >> (128 - len as u32)),
        _ => false,
    }
}

fn trap_addr(s: &str) -> Option<SocketAddr> {
    s.parse::<SocketAddr>().ok().or_else(|| s.parse::<IpAddr>().ok().map(|ip| SocketAddr::new(ip, 162)))
}

impl SnmpSettings {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("snmp.json")
    }

    pub fn load(data_dir: &Path) -> Self {
        std::fs::read_to_string(Self::path(data_dir)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn check(&mut self) -> Result<(), String> {
        self.community = self.community.trim().to_string();
        let clean = |v: &mut Vec<String>| {
            *v = v.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        };
        clean(&mut self.allowed);
        clean(&mut self.traps);
        if self.enabled && self.community.is_empty() {
            return Err("give a community".into());
        }
        if self.community.len() > 64 || !self.community.chars().all(|c| c.is_ascii_graphic()) {
            return Err("the community is letters, digits and signs, without spaces".into());
        }
        if self.port == 0 {
            return Err("give a port".into());
        }
        if let Some(a) = self.allowed.iter().find(|a| net_of(a).is_none()) {
            return Err(format!("not an address or a network: {a}"));
        }
        if let Some(t) = self.traps.iter().find(|t| trap_addr(t).is_none()) {
            return Err(format!("not an address for the traps: {t}"));
        }
        self.contact = self.contact.trim().chars().take(200).collect();
        self.location = self.location.trim().chars().take(200).collect();
        Ok(())
    }

    fn masked(&self) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        if !self.community.is_empty() {
            v["community"] = json!(MASK);
        }
        v
    }
}

// ---------- BER (repris de Strata Network) ----------

fn put_len(out: &mut Vec<u8>, len: usize) {
    if len < 0x80 {
        out.push(len as u8);
    } else {
        let bytes = len.to_be_bytes();
        let skip = bytes.iter().take_while(|b| **b == 0).count();
        out.push(0x80 | (bytes.len() - skip) as u8);
        out.extend_from_slice(&bytes[skip..]);
    }
}

fn tlv(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    put_len(&mut out, body.len());
    out.extend_from_slice(body);
    out
}

fn int(v: i64) -> Vec<u8> {
    let bytes = v.to_be_bytes();
    let mut start = 0;
    while start < 7 && ((bytes[start] == 0 && bytes[start + 1] & 0x80 == 0) || (bytes[start] == 0xFF && bytes[start + 1] & 0x80 != 0)) {
        start += 1;
    }
    tlv(0x02, &bytes[start..])
}

fn uint(tag: u8, v: u64) -> Vec<u8> {
    let bytes = v.to_be_bytes();
    let skip = bytes.iter().take_while(|b| **b == 0).count().min(7);
    // Un octet de tête à 0 si le premier bit est levé (sinon négatif).
    let mut body = if bytes[skip] & 0x80 != 0 { vec![0] } else { Vec::new() };
    body.extend_from_slice(&bytes[skip..]);
    tlv(tag, &body)
}

fn oid_bytes(o: &[u32]) -> Vec<u8> {
    let mut body = Vec::new();
    if o.len() >= 2 {
        body.push((o[0] * 40 + o[1]) as u8);
    }
    for &n in o.iter().skip(2) {
        let mut stack = vec![(n & 0x7F) as u8];
        let mut n = n >> 7;
        while n > 0 {
            stack.push(0x80 | (n & 0x7F) as u8);
            n >>= 7;
        }
        body.extend(stack.iter().rev());
    }
    tlv(0x06, &body)
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn next(&mut self) -> Option<(u8, &'a [u8])> {
        let tag = *self.b.get(self.pos)?;
        let mut len = usize::from(*self.b.get(self.pos + 1)?);
        let mut p = self.pos + 2;
        if len & 0x80 != 0 {
            let n = len & 0x7F;
            if n > 4 {
                return None;
            }
            len = 0;
            for i in 0..n {
                len = (len << 8) | usize::from(*self.b.get(p + i)?);
            }
            p += n;
        }
        let body = self.b.get(p..p + len)?;
        self.pos = p + len;
        Some((tag, body))
    }
}

fn read_int(b: &[u8]) -> i64 {
    let mut v: i64 = if b.first().is_some_and(|x| x & 0x80 != 0) { -1 } else { 0 };
    for x in b.iter().take(8) {
        v = (v << 8) | i64::from(*x);
    }
    v
}

fn read_oid(b: &[u8]) -> Vec<u32> {
    let mut v = Vec::new();
    if let Some(first) = b.first() {
        v.push(u32::from(*first / 40));
        v.push(u32::from(*first % 40));
    }
    let mut n: u32 = 0;
    for x in b.iter().skip(1) {
        n = (n << 7) | u32::from(x & 0x7F);
        if x & 0x80 == 0 {
            v.push(n);
            n = 0;
        }
    }
    v
}

// ---------- Les valeurs ----------

#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Int(i64),
    Str(String),
    Gauge(u32),
    Counter(u32),
    Ticks(u32),
    Oid(Vec<u32>),
}

impl Val {
    fn ber(&self) -> Vec<u8> {
        match self {
            Val::Int(i) => int(*i),
            Val::Str(s) => tlv(0x04, s.as_bytes()),
            Val::Gauge(g) => uint(0x42, u64::from(*g)),
            Val::Counter(c) => uint(0x41, u64::from(*c)),
            Val::Ticks(t) => uint(0x43, u64::from(*t)),
            Val::Oid(o) => oid_bytes(o),
        }
    }

    pub fn text(&self) -> String {
        match self {
            Val::Int(i) => i.to_string(),
            Val::Str(s) => s.clone(),
            Val::Gauge(g) | Val::Counter(g) | Val::Ticks(g) => g.to_string(),
            Val::Oid(o) => o.iter().map(u32::to_string).collect::<Vec<_>>().join("."),
        }
    }
}

fn under(path: &[u32]) -> Vec<u32> {
    [ROOT, path].concat()
}

/// Une ligne de table : son rang, et (colonne, nom, valeur).
type Row<'a> = (u32, Vec<(u32, &'a str, Val)>);
/// Ce que l'agent sait : (oid, nom, valeur).
pub type View = Vec<(Vec<u32>, String, Val)>;

const SYS: [u32; 7] = [1, 3, 6, 1, 2, 1, 1];

/// Tout ce que l'agent sait, trié par OID : (oid, nom, valeur).
type Walk = Vec<(Vec<u32>, String, Val)>;

/// La MIB d'il y a au plus une seconde : un `snmpwalk` envoie des centaines de requêtes, une seule
/// reconstruction suffit.
fn recent_snapshot(s: &AppState) -> std::sync::Arc<Walk> {
    type Last = (std::time::Instant, std::path::PathBuf, std::sync::Arc<Walk>);
    static LAST: Mutex<Option<Last>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, dir, w)) = last.as_ref() {
        if *dir == s.data_dir && at.elapsed() < std::time::Duration::from_secs(1) {
            return w.clone();
        }
    }
    let w = std::sync::Arc::new(snapshot(s));
    *last = Some((std::time::Instant::now(), s.data_dir.clone(), w.clone()));
    w
}

pub fn snapshot(s: &AppState) -> Vec<(Vec<u32>, String, Val)> {
    let mut v: Vec<(Vec<u32>, String, Val)> = Vec::new();
    let set = s.snmp_settings.lock().unwrap().clone();
    let cfg = s.config.load();
    let up = (crate::health::uptime_secs().max(0) as u64 * 100).min(u32::MAX as u64) as u32;
    let sys = |n: u32| [&SYS[..], &[n, 0]].concat();
    v.push((sys(1), "sysDescr".into(), Val::Str(format!("Strata Core {} · {}", strata_core::VERSION, sysinfo::System::long_os_version().unwrap_or_default()))));
    v.push((sys(2), "sysObjectID".into(), Val::Oid(ROOT.to_vec())));
    v.push((sys(3), "sysUpTime".into(), Val::Ticks(up)));
    v.push((sys(4), "sysContact".into(), Val::Str(set.contact.clone())));
    v.push((sys(5), "sysName".into(), Val::Str(cfg.dns_name.clone())));
    v.push((sys(6), "sysLocation".into(), Val::Str(set.location.clone())));
    v.push((sys(7), "sysServices".into(), Val::Int(72)));

    // Le Core.
    let now = s.resources.now();
    let problems = s.alerts.active();
    let clock = s.clock.get();
    let pct = |a: u64, b: u64| if b > 0 { (a as f64 / b as f64 * 100.0).round() as u32 } else { 0 };
    let tri = |b: Option<bool>| {
        Val::Int(match b {
            Some(true) => 1,
            Some(false) => 2,
            None => 3,
        })
    };
    let core = |n: u32| under(&[1, n, 0]);
    v.push((core(1), "coreVersion".into(), Val::Str(strata_core::VERSION.into())));
    v.push((core(2), "coreServerName".into(), Val::Str(cfg.name.clone())));
    v.push((core(3), "coreUptime".into(), Val::Ticks(up)));
    v.push((core(4), "coreProblems".into(), Val::Gauge(problems.len() as u32)));
    v.push((core(5), "coreCpu".into(), Val::Gauge(now.as_ref().map_or(0, |n| n.cpu.round() as u32))));
    v.push((core(6), "coreMemory".into(), Val::Gauge(now.as_ref().map_or(0, |n| pct(n.mem_used, n.mem_total)))));
    let disk = now.as_ref().and_then(|n| n.disks.iter().find(|d| d.data).map(|d| pct(d.total - d.free.min(d.total), d.total)));
    v.push((core(7), "coreDataDisk".into(), Val::Gauge(disk.unwrap_or(0))));
    v.push((core(8), "coreTimeSynced".into(), tri(clock.ntp.synced)));
    v.push((core(9), "corePtpLocked".into(), tri(clock.ptp.running.then_some(clock.ptp.locked))));

    // Les applis.
    let apps = s.apps.list();
    let uses = now.as_ref().map(|n| n.uses.clone()).unwrap_or_default();
    let mut rows: Vec<Row> = Vec::new();
    for (i, a) in apps.iter().enumerate() {
        let st = s.supervisor.status(&a.manifest.id);
        let state = if !a.active {
            5
        } else {
            match st.state {
                crate::supervisor::RunState::Stopped => 1,
                crate::supervisor::RunState::Starting => 2,
                crate::supervisor::RunState::Running => 3,
                crate::supervisor::RunState::Crashed => 4,
            }
        };
        let u = uses.iter().find(|u| u.id == a.manifest.id);
        rows.push((
            i as u32 + 1,
            vec![
                (2, "appId", Val::Str(a.manifest.id.clone())),
                (3, "appName", Val::Str(a.manifest.name.clone())),
                (4, "appVersion", Val::Str(a.manifest.version.to_string())),
                (5, "appState", Val::Int(state)),
                (6, "appCpu", Val::Gauge(u.map_or(0, |u| (u.cores * 100.0).round() as u32))),
                (7, "appMemory", Val::Gauge(u.map_or(0, |u| (u.memory / 1024).min(u32::MAX as u64) as u32))),
                (8, "appRestarts", Val::Counter(st.restarts)),
            ],
        ));
    }
    table(&mut v, 2, rows);

    // Les entrées SDI.
    let hw = crate::hardware::HardwareConfig::load(&s.data_dir);
    let mut rows = Vec::new();
    for (i, inp) in s.captures.list().iter().enumerate() {
        let card = hw.cards.iter().find(|c| c.id == inp.card);
        let name = card.and_then(|c| c.connectors.get(inp.connector as usize - 1)).map(|k| k.name.clone()).unwrap_or_default();
        rows.push((
            i as u32 + 1,
            vec![
                (
                    2,
                    "inputCard",
                    Val::Str(card.map(|c| if c.name.is_empty() { c.model_name.clone() } else { c.name.clone() }).unwrap_or_else(|| inp.card.clone())),
                ),
                (3, "inputConnector", Val::Int(inp.connector as i64)),
                (4, "inputName", Val::Str(name)),
                (5, "inputSignal", tri(inp.signal)),
                (6, "inputFormat", Val::Str(inp.format.clone().unwrap_or_default())),
                (7, "inputFps", Val::Gauge((inp.fps * 100.0).round() as u32)),
                (8, "inputReaders", Val::Gauge(inp.readers.len() as u32)),
            ],
        ));
    }
    table(&mut v, 4, rows);

    // Les problèmes en cours.
    let now_ms = crate::logs::now_ms();
    let mut rows = Vec::new();
    for (i, p) in problems.iter().enumerate() {
        rows.push((
            i as u32 + 1,
            vec![
                (2, "problemKey", Val::Str(p.key.clone())),
                (3, "problemText", Val::Str(p.text.clone())),
                (4, "problemAge", Val::Ticks(((now_ms - p.since).max(0) / 10).min(u32::MAX as i64) as u32)),
            ],
        ));
    }
    table(&mut v, 3, rows);
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

/// Une table : `<racine>.<n>.1.<colonne>.<rang>`, avec la colonne 1 = le rang.
fn table(v: &mut View, n: u32, rows: Vec<Row>) {
    for (idx, cols) in rows {
        let prefix = match n {
            2 => "app",
            3 => "problem",
            _ => "input",
        };
        v.push((under(&[n, 1, 1, idx]), format!("{prefix}Index.{idx}"), Val::Int(idx as i64)));
        for (c, name, val) in cols {
            v.push((under(&[n, 1, c, idx]), format!("{name}.{idx}"), val));
        }
    }
}

// ---------- L'agent ----------

/// La réponse à une requête (None : pas pour nous, ou illisible).
pub fn answer(req: &[u8], community: &str, view: &[(Vec<u32>, String, Val)]) -> Option<Vec<u8>> {
    let (tag, msg) = Reader { b: req, pos: 0 }.next()?;
    if tag != 0x30 {
        return None;
    }
    let mut r = Reader { b: msg, pos: 0 };
    let (_, ver) = r.next()?;
    let version = read_int(ver);
    let (_, comm) = r.next()?;
    // v2c seulement, et la bonne communauté (comparée en temps constant).
    if version != 1 || comm.len() != community.len() || comm.iter().zip(community.as_bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) != 0 {
        return None;
    }
    let (pdu, body) = r.next()?;
    let mut p = Reader { b: body, pos: 0 };
    let id = read_int(p.next()?.1);
    let a = read_int(p.next()?.1);
    let b = read_int(p.next()?.1);
    let (_, binds) = p.next()?;
    let mut list = Reader { b: binds, pos: 0 };
    let mut oids = Vec::new();
    while let Some((_, bind)) = list.next() {
        let (_, o) = Reader { b: bind, pos: 0 }.next()?;
        oids.push(read_oid(o));
        if oids.len() > 64 {
            break;
        }
    }
    let exact = |o: &Vec<u32>| -> Vec<u8> {
        match view.iter().find(|(k, _, _)| k == o) {
            Some((_, _, v)) => v.ber(),
            // noSuchInstance : l'objet existe, pas cette instance ; sinon noSuchObject.
            None if !o.is_empty() && view.iter().any(|(k, _, _)| k.len() == o.len() && k[..k.len() - 1] == o[..o.len() - 1]) => vec![0x81, 0],
            None => vec![0x80, 0],
        }
    };
    let next = |o: &Vec<u32>| -> (Vec<u32>, Vec<u8>) {
        match view.iter().find(|(k, _, _)| k > o) {
            Some((k, _, v)) => (k.clone(), v.ber()),
            None => (o.clone(), vec![0x82, 0]),
        }
    };
    let mut out: Vec<(Vec<u32>, Vec<u8>)> = Vec::new();
    match pdu {
        0xA0 => out.extend(oids.iter().map(|o| (o.clone(), exact(o)))),
        0xA1 => out.extend(oids.iter().map(next)),
        0xA5 => {
            let non_rep = (a.max(0) as usize).min(oids.len());
            let max_rep = b.clamp(0, 50) as usize;
            out.extend(oids[..non_rep].iter().map(next));
            let mut cur: Vec<Vec<u32>> = oids[non_rep..].to_vec();
            for _ in 0..max_rep {
                if cur.is_empty() {
                    break;
                }
                let mut done = true;
                for c in cur.iter_mut() {
                    let (k, v) = next(c);
                    if v[0] != 0x82 {
                        done = false;
                    }
                    *c = k.clone();
                    out.push((k, v));
                }
                if done || out.len() > 400 {
                    break;
                }
            }
        }
        _ => return None,
    }
    let binds: Vec<u8> = out.iter().flat_map(|(o, v)| tlv(0x30, &[oid_bytes(o), v.clone()].concat())).collect();
    let resp = tlv(0xA2, &[int(id), int(0), int(0), tlv(0x30, &binds)].concat());
    Some(tlv(0x30, &[int(1), tlv(0x04, community.as_bytes()), resp].concat()))
}

/// Un trap v2c : le problème (`problem`) ou sa fin (`ok`).
pub fn trap_bytes(community: &str, uptime_ticks: u32, level: &str, key: &str, text: &str) -> Vec<u8> {
    let kind = under(&[0, if level == "ok" { 2 } else { 1 }]);
    let binds = [
        tlv(0x30, &[oid_bytes(&[1, 3, 6, 1, 2, 1, 1, 3, 0]), uint(0x43, u64::from(uptime_ticks))].concat()),
        tlv(0x30, &[oid_bytes(&[1, 3, 6, 1, 6, 3, 1, 1, 4, 1, 0]), oid_bytes(&kind)].concat()),
        tlv(0x30, &[oid_bytes(&under(&[5, 1, 0])), tlv(0x04, key.as_bytes())].concat()),
        tlv(0x30, &[oid_bytes(&under(&[5, 2, 0])), tlv(0x04, text.as_bytes())].concat()),
    ]
    .concat();
    let id = crate::logs::now_ms() & 0x7fff_ffff;
    let pdu = tlv(0xA7, &[int(id), int(0), int(0), tlv(0x30, &binds)].concat());
    tlv(0x30, &[int(1), tlv(0x04, community.as_bytes()), pdu].concat())
}

/// Envoie un trap aux destinations réglées (appelé avec les alertes) : faux s'il n'y en a pas.
pub fn send_trap(s: &AppState, level: &str, key: &str, text: &str) -> bool {
    let set = s.snmp_settings.lock().unwrap().clone();
    if !set.enabled || set.traps.is_empty() {
        return false;
    }
    let up = (crate::health::uptime_secs().max(0) as u64 * 100).min(u32::MAX as u64) as u32;
    let bytes = trap_bytes(&set.community, up, level, key, text);
    if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0") {
        for t in set.traps.iter().filter_map(|t| trap_addr(t)) {
            if let Err(e) = sock.send_to(&bytes, t) {
                tracing::warn!("SNMP trap to {t}: {e}");
            }
        }
    }
    true
}

/// L'agent en marche.
#[derive(Default)]
pub struct Agent {
    task: Mutex<Option<(JoinHandle<()>, SocketAddr)>>,
    /// Un `apply` à la fois.
    applying: tokio::sync::Mutex<()>,
    pub error: Mutex<Option<String>>,
}

impl Agent {
    pub fn listening(&self) -> Option<SocketAddr> {
        self.task.lock().unwrap().as_ref().map(|(_, a)| *a)
    }
}

/// (Re)lance l'agent selon les réglages.
pub async fn apply(s: &Shared) {
    let _one = s.snmp.applying.lock().await;
    let old = s.snmp.task.lock().unwrap().take();
    if let Some((t, _)) = old {
        // Fini avant de rouvrir le port : sa socket est alors fermée.
        t.abort();
        let _ = t.await;
    }
    *s.snmp.error.lock().unwrap() = None;
    let set = s.snmp_settings.lock().unwrap().clone();
    if !set.enabled {
        return;
    }
    let sock = match UdpSocket::bind(("0.0.0.0", set.port)).await {
        Ok(sock) => sock,
        Err(e) => {
            let why = format!("cannot listen on UDP port {}: {e}", set.port);
            tracing::warn!("SNMP: {why}");
            *s.snmp.error.lock().unwrap() = Some(why);
            return;
        }
    };
    let addr = sock.local_addr().unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], set.port)));
    tracing::info!("SNMP agent on UDP {addr}");
    let weak: Weak<AppState> = Arc::downgrade(s);
    let task = tokio::spawn(async move {
        let mut buf = vec![0u8; 4096];
        loop {
            let Ok((n, from)) = sock.recv_from(&mut buf).await else { continue };
            let Some(s) = weak.upgrade() else { return };
            let set = s.snmp_settings.lock().unwrap().clone();
            let allowed = set.allowed.is_empty() || set.allowed.iter().filter_map(|a| net_of(a)).any(|net| in_net(from.ip(), net));
            if !allowed {
                continue;
            }
            let req = buf[..n].to_vec();
            let s2 = s.clone();
            let Ok(Some(resp)) = tokio::task::spawn_blocking(move || answer(&req, &set.community, &recent_snapshot(&s2))).await else { continue };
            let _ = sock.send_to(&resp, from).await;
        }
    });
    *s.snmp.task.lock().unwrap() = Some((task, addr));
}

// ---------- Les routes ----------

pub fn routes() -> Router<Shared> {
    Router::new().route("/api/health/snmp", get(read).put(write)).route("/api/health/snmp/mib", get(mib))
}

async fn read(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let s2 = s.clone();
    let walk = crate::api::blocking(move || snapshot(&s2)).await?;
    let root = ROOT.iter().map(u32::to_string).collect::<Vec<_>>().join(".");
    Ok(Json(json!({
        "settings": s.snmp_settings.lock().unwrap().masked(),
        "listening": s.snmp.listening(),
        "error": s.snmp.error.lock().unwrap().clone(),
        "root": root,
        "walk": walk.iter().map(|(o, n, v)| json!({"oid": o.iter().map(u32::to_string).collect::<Vec<_>>().join("."), "name": n, "value": v.text()})).collect::<Vec<_>>(),
    })))
}

async fn write(State(s): State<Shared>, CurrentUser(me): CurrentUser, Json(mut b): Json<SnmpSettings>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    if b.community == MASK {
        b.community = s.snmp_settings.lock().unwrap().community.clone();
    }
    b.check().map_err(bad)?;
    strata_core::files::write_json(&SnmpSettings::path(&s.data_dir), &b, true).map_err(|e| bad(format!("cannot save the SNMP settings: {e}")))?;
    *s.snmp_settings.lock().unwrap() = b.clone();
    apply(&s).await;
    tracing::info!("SNMP settings changed by {}", me.username);
    let error = s.snmp.error.lock().unwrap().clone();
    Ok(Json(json!({ "settings": b.masked(), "listening": s.snmp.listening(), "error": error })))
}

/// La MIB, pour les outils de supervision.
async fn mib(CurrentUser(me): CurrentUser) -> ApiResult<impl IntoResponse> {
    require(me.admin)?;
    Ok((
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8"), (header::CONTENT_DISPOSITION, "attachment; filename=\"STRATA-CORE-MIB.txt\"")],
        include_str!("../STRATA-CORE-MIB.txt"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Une requête v2c (comme celles de Strata Network).
    fn request(community: &str, pdu: u8, oids: &[Vec<u32>], non_rep: i64, max_rep: i64) -> Vec<u8> {
        let binds: Vec<u8> = oids.iter().flat_map(|o| tlv(0x30, &[oid_bytes(o), vec![0x05, 0]].concat())).collect();
        let p = tlv(pdu, &[int(42), int(non_rep), int(max_rep), tlv(0x30, &binds)].concat());
        tlv(0x30, &[int(1), tlv(0x04, community.as_bytes()), p].concat())
    }

    fn binds(resp: &[u8]) -> Vec<(Vec<u32>, u8, Vec<u8>)> {
        let (_, msg) = Reader { b: resp, pos: 0 }.next().unwrap();
        let mut r = Reader { b: msg, pos: 0 };
        r.next();
        r.next();
        let (pdu, body) = r.next().unwrap();
        assert_eq!(pdu, 0xA2);
        let mut p = Reader { b: body, pos: 0 };
        assert_eq!(read_int(p.next().unwrap().1), 42);
        p.next();
        p.next();
        let (_, list) = p.next().unwrap();
        let mut l = Reader { b: list, pos: 0 };
        let mut out = Vec::new();
        while let Some((_, b)) = l.next() {
            let mut v = Reader { b, pos: 0 };
            let (_, o) = v.next().unwrap();
            let (t, val) = v.next().unwrap();
            out.push((read_oid(o), t, val.to_vec()));
        }
        out
    }

    fn view() -> Vec<(Vec<u32>, String, Val)> {
        let mut v = vec![
            ([&SYS[..], &[5, 0]].concat(), "sysName".into(), Val::Str("strata01".into())),
            (under(&[1, 4, 0]), "coreProblems".into(), Val::Gauge(2)),
            (under(&[1, 3, 0]), "coreUptime".into(), Val::Ticks(300_000)),
        ];
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    #[test]
    fn get_next_bulk_and_community() {
        let v = view();
        let sys_name = [&SYS[..], &[5, 0]].concat();
        let r = answer(&request("regie", 0xA0, &[sys_name.clone(), under(&[1, 4, 1]), vec![1, 3, 6, 1, 9]], 0, 0), "regie", &v).unwrap();
        let b = binds(&r);
        assert_eq!((b[0].1, b[0].2.as_slice()), (0x04, b"strata01".as_slice()));
        assert_eq!(b[1].1, 0x81, "noSuchInstance inside our tree");
        assert_eq!(b[2].1, 0x80, "noSuchObject elsewhere");
        // GETNEXT depuis le début : la première valeur.
        let b = binds(&answer(&request("regie", 0xA1, &[vec![1, 3]], 0, 0), "regie", &v).unwrap());
        assert_eq!(b[0].0, sys_name);
        // GETBULK : tout, puis la fin de la vue.
        let b = binds(&answer(&request("regie", 0xA5, &[vec![1, 3]], 0, 10), "regie", &v).unwrap());
        assert_eq!(b.len(), 4);
        assert_eq!((b[1].0.clone(), b[1].1, b[1].2.clone()), (under(&[1, 3, 0]), 0x43, vec![0x04, 0x93, 0xE0]));
        assert_eq!(b[3].1, 0x82);
        // Une autre communauté, ou SNMP v1 : pas de réponse.
        assert!(answer(&request("public", 0xA0, std::slice::from_ref(&sys_name), 0, 0), "regie", &v).is_none());
        let mut v1 = request("regie", 0xA0, &[sys_name], 0, 0);
        v1[4] = 0;
        assert!(answer(&v1, "regie", &v).is_none());
    }

    #[test]
    fn settings_and_networks() {
        let mut s = SnmpSettings { enabled: true, ..SnmpSettings::default() };
        assert_eq!(s.check().unwrap_err(), "give a community");
        s.community = "regie".into();
        s.allowed = vec![" 10.0.0.0/24 ".into(), "".into()];
        s.traps = vec!["10.0.0.9".into(), "10.0.0.9:1162".into()];
        s.check().unwrap();
        assert_eq!(s.allowed, ["10.0.0.0/24"]);
        assert!(in_net("10.0.0.77".parse().unwrap(), net_of("10.0.0.0/24").unwrap()));
        assert!(!in_net("10.0.1.77".parse().unwrap(), net_of("10.0.0.0/24").unwrap()));
        assert_eq!(trap_addr("10.0.0.9"), Some("10.0.0.9:162".parse().unwrap()));
        s.allowed = vec!["pas une adresse".into()];
        assert!(s.check().is_err());
        let t = trap_bytes("regie", 100, "problem", "disk:/", "Disque / presque plein");
        assert_eq!(t[0], 0x30);
    }
}
