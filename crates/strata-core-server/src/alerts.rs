//! Les alertes (Réglages › Santé › Alertes), reprises de Cut : ce qui va mal, dit hors du serveur
//! par un webhook (Slack, Teams, Discord, Mattermost, ou tout service qui lit du JSON) et par
//! e-mail, puis dit de nouveau quand c'est fini.
//!
//! Toutes les 5 s, le Core regarde : disques presque pleins, mémoire et processeurs saturés, une
//! appli active qui ne tourne plus, une carte DeckLink réglée qui n'est plus vue (après une mise à
//! jour du noyau, par exemple), une REF perdue, une entrée SDI sans signal, l'heure qui ne suit plus
//! le PTP, un redémarrage après un arrêt inattendu, et les alertes que les applis déclarent.
//! Un problème est dit quand il a duré `delay_secs` (un évènement ponctuel, tout de suite).
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use arc_swap::ArcSwap;
use axum::{
    extract::{ConnectInfo, State},
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::SocketAddr;

use crate::{
    api::{bad, blocking, require, ApiResult},
    auth::{CurrentUser, MaybeUser},
    guard,
    hardware::{self, HardwareConfig},
    logs::now_ms,
    sso::MASK,
    state::{AppState, Shared},
};

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct AlertSettings {
    pub enabled: bool,
    /// La langue des messages : `fr` ou `en`.
    pub language: String,
    /// Chaque canal s'allume à part.
    pub webhook_enabled: bool,
    pub email_enabled: bool,
    /// Les traps SNMP, vers les destinations de Santé › SNMP.
    pub snmp_traps: bool,
    /// Reçoit un message JSON (`text` et `content` : ce qu'affichent les messageries).
    pub webhook: String,
    /// Destinataires, séparés par des virgules.
    pub email_to: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    /// `starttls`, `tls` (dès le premier octet) ou `none`.
    pub smtp_security: String,
    pub smtp_user: String,
    pub smtp_password: String,
    pub smtp_from: String,
    /// Un problème est dit quand il a duré ce temps (s).
    pub delay_secs: u32,
    /// Ce qui est dit.
    pub system: bool,
    pub apps: bool,
    pub hardware: bool,
    pub clock: bool,
    pub restart: bool,
}

impl Default for AlertSettings {
    fn default() -> Self {
        AlertSettings {
            enabled: false,
            language: "fr".into(),
            webhook_enabled: true,
            email_enabled: true,
            snmp_traps: true,
            webhook: String::new(),
            email_to: String::new(),
            smtp_host: String::new(),
            smtp_port: 587,
            smtp_security: "starttls".into(),
            smtp_user: String::new(),
            smtp_password: String::new(),
            smtp_from: String::new(),
            delay_secs: 30,
            system: true,
            apps: true,
            hardware: true,
            clock: true,
            restart: true,
        }
    }
}

fn recipients(list: &str) -> Vec<String> {
    list.split([',', ';', ' ', '\n']).map(str::trim).filter(|a| !a.is_empty()).map(String::from).collect()
}

/// Ressemble à une adresse, et rien qui puisse casser une commande SMTP.
fn plausible(a: &str) -> bool {
    a.chars().all(|c| !c.is_whitespace() && !c.is_control() && c != '<' && c != '>') && a.split_once('@').is_some_and(|(u, d)| !u.is_empty() && d.contains('.'))
}

impl AlertSettings {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("alerts.json")
    }

    pub fn load(data_dir: &Path) -> Self {
        std::fs::read_to_string(Self::path(data_dir)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn check(&mut self) -> Result<(), String> {
        for s in [&mut self.webhook, &mut self.email_to, &mut self.smtp_host, &mut self.smtp_user, &mut self.smtp_from] {
            *s = s.trim().to_string();
        }
        if self.language != "en" {
            self.language = "fr".into();
        }
        if !self.webhook.is_empty() && !(self.webhook.starts_with("https://") || self.webhook.starts_with("http://")) {
            return Err("the webhook address starts with https://".into());
        }
        if !["starttls", "tls", "none"].contains(&self.smtp_security.as_str()) {
            return Err("the mail server's security is starttls, tls or none".into());
        }
        if self.email_enabled && !self.email_to.is_empty() {
            if self.smtp_host.is_empty() || self.smtp_from.is_empty() {
                return Err("give the mail server and the sender's address".into());
            }
            if self.smtp_port == 0 {
                return Err("give the mail server's port".into());
            }
            if recipients(&self.email_to).iter().chain(std::iter::once(&self.smtp_from)).any(|a| !plausible(a)) {
                return Err("an e-mail address is not valid".into());
            }
        }
        if !(5..=3600).contains(&self.delay_secs) {
            return Err("the delay before an alert is 5 to 3600 s".into());
        }
        Ok(())
    }

    fn fr(&self) -> bool {
        self.language != "en"
    }

    /// Pour le navigateur : le mot de passe masqué.
    fn masked(&self) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        if !self.smtp_password.is_empty() {
            v["smtpPassword"] = json!(MASK);
        }
        v
    }
}

/// Un problème vu maintenant.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    /// Un par chose (`disk:/`, `app:livecut`, `card:7b10a04e`…).
    pub key: String,
    /// `system`, `apps`, `hardware`, `clock`, `restart`, `app`.
    pub event: &'static str,
    pub since: i64,
    pub text: String,
    #[serde(skip)]
    over: String,
    /// Ponctuel : dit tout de suite, une fois.
    #[serde(skip)]
    once: bool,
    /// Déjà dit.
    pub told: bool,
}

/// Un message envoyé, et comment.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Sent {
    pub at: i64,
    /// `problem`, `ok` ou `info`.
    pub level: &'static str,
    pub text: String,
    /// Par canal : vide si bien envoyé, sinon pourquoi.
    pub results: Vec<(String, Option<String>)>,
}

/// Une alerte déclarée par une appli.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct Declared {
    app: String,
    key: String,
    #[serde(default = "yes")]
    active: bool,
    #[serde(default)]
    text: String,
    #[serde(default)]
    over: String,
}

/// Une alerte d'appli : depuis quand, son texte, celui de sa fin.
type DeclaredState = (i64, String, String);

fn yes() -> bool {
    true
}

/// L'état des alertes.
pub struct Alerts {
    pub settings: ArcSwap<AlertSettings>,
    /// Les problèmes vus au dernier tour.
    active: Mutex<Vec<Problem>>,
    /// Les derniers messages.
    recent: Mutex<VecDeque<Sent>>,
    /// Les alertes des applis : (appli, clé) → (depuis, texte, fin).
    declared: Mutex<HashMap<(String, String), DeclaredState>>,
    /// Depuis quand chaque appli active ne tourne plus.
    down: Mutex<HashMap<String, i64>>,
    restart_told: Mutex<bool>,
}

impl Alerts {
    pub fn new(data_dir: &Path) -> Self {
        Alerts {
            settings: ArcSwap::from_pointee(AlertSettings::load(data_dir)),
            active: Mutex::default(),
            recent: Mutex::default(),
            declared: Mutex::default(),
            down: Mutex::default(),
            restart_told: Mutex::new(false),
        }
    }

    pub fn active(&self) -> Vec<Problem> {
        self.active.lock().unwrap().clone()
    }
}

// ---------- Ce qui va mal ----------

/// Les problèmes vus maintenant (bloquant : lit le matériel et l'horloge).
fn look(s: &AppState) -> Vec<Problem> {
    let a = s.alerts.settings.load_full();
    let fr = a.fr();
    let say = |f: String, e: String| if fr { f } else { e };
    let now = now_ms();
    let mut out: Vec<Problem> = Vec::new();
    let mut push = |key: String, event: &'static str, since: i64, once: bool, text: String, over: String| {
        out.push(Problem { key, event, since, text, over, once, told: false })
    };

    if a.system {
        if let Some(r) = s.resources.now() {
            for d in &r.disks {
                let full = if d.total > 0 { (d.total - d.free.min(d.total)) as f64 / d.total as f64 * 100.0 } else { 0.0 };
                if full >= 95.0 {
                    push(
                        format!("disk:{}", d.mount),
                        "system",
                        now,
                        false,
                        say(format!("Disque {} presque plein : {full:.0} %", d.mount), format!("Disk {} almost full: {full:.0} %", d.mount)),
                        say(format!("Le disque {} a de nouveau de la place", d.mount), format!("Disk {} has room again", d.mount)),
                    );
                }
            }
            let mem = if r.mem_total > 0 { r.mem_used as f64 / r.mem_total as f64 * 100.0 } else { 0.0 };
            if mem >= 95.0 {
                push(
                    "memory".into(),
                    "system",
                    now,
                    false,
                    say(format!("Mémoire presque pleine : {mem:.0} %"), format!("Memory almost full: {mem:.0} %")),
                    say("La mémoire a de nouveau de la place".into(), "Memory has room again".into()),
                );
            }
            if r.cpu >= 95.0 {
                push(
                    "cpu".into(),
                    "system",
                    now,
                    false,
                    say(format!("Processeurs saturés : {:.0} %", r.cpu), format!("Processors saturated: {:.0} %", r.cpu)),
                    say("Les processeurs ne sont plus saturés".into(), "Processors no longer saturated".into()),
                );
            }
        }
    }

    if a.apps {
        let mut down = s.alerts.down.lock().unwrap();
        let mut still = HashMap::new();
        for app in s.apps.list().into_iter().filter(|x| x.active && x.manifest.run.is_some()) {
            let st = s.supervisor.status(&app.manifest.id);
            if st.state == crate::supervisor::RunState::Running {
                continue;
            }
            let since = *down.get(&app.manifest.id).unwrap_or(&now);
            still.insert(app.manifest.id.clone(), since);
            let name = &app.manifest.name;
            let why = st.exit.map(|e| format!(" ({e})")).unwrap_or_default();
            push(
                format!("app:{}", app.manifest.id),
                "apps",
                since,
                false,
                say(format!("{name} ne tourne plus{why}"), format!("{name} is not running{why}")),
                say(format!("{name} tourne de nouveau"), format!("{name} runs again")),
            );
        }
        *down = still;
    }

    if a.hardware {
        let cfg = HardwareConfig::load(&s.data_dir);
        let (cards, _, _) = hardware::decklink::detect();
        for c in &cfg.cards {
            let name = if c.name.is_empty() { c.model_name.clone() } else { format!("« {} » ({})", c.name, c.model_name) };
            match cards.iter().find(|d| d.id == c.id) {
                None => push(
                    format!("card:{}", c.id),
                    "hardware",
                    now,
                    false,
                    say(
                        format!("Carte DeckLink {name} plus vue : pilote Desktop Video arrêté, ou noyau mis à jour ?"),
                        format!("DeckLink card {name} no longer seen: Desktop Video stopped, or kernel updated?"),
                    ),
                    say(format!("Carte DeckLink {name} de nouveau vue"), format!("DeckLink card {name} seen again")),
                ),
                Some(d) if c.genlock && d.has_reference && d.reference_locked == Some(false) => push(
                    format!("ref:{}", c.id),
                    "hardware",
                    now,
                    false,
                    say(format!("{name} : REF perdue, sorties non calées"), format!("{name}: REF lost, outputs not genlocked")),
                    say(format!("{name} : REF de nouveau verrouillée"), format!("{name}: REF locked again")),
                ),
                _ => {}
            }
        }
        for i in s.captures.list() {
            let card = cfg.cards.iter().find(|c| c.id == i.card);
            let wire = card.and_then(|c| c.connectors.get(i.connector as usize - 1)).map(|k| k.name.clone()).filter(|n| !n.is_empty());
            let label = wire.map_or_else(|| format!("SDI {}", i.connector), |w| format!("SDI {} · {w}", i.connector));
            if i.signal == Some(false) || i.state == hardware::capture::CaptureState::Error {
                push(
                    format!("input:{}:{}", i.card, i.connector),
                    "hardware",
                    now,
                    false,
                    say(format!("{label} : pas de signal"), format!("{label}: no signal")),
                    say(format!("{label} : signal revenu"), format!("{label}: signal back")),
                );
            }
        }
    }

    // Les flux réseau : une entrée qui ne reçoit plus, une destination qui n'envoie plus.
    if a.hardware {
        let cfg = crate::streams::StreamsConfig::load(&s.data_dir);
        use crate::streams::engine::State as St;
        for i in cfg.inputs.iter().filter(|i| i.enabled) {
            if let Some(st) = s.streams.input_status(&i.id).filter(|x| matches!(x.state, St::Error | St::Waiting)) {
                let why = st.error.clone().unwrap_or_default();
                push(
                    format!("stream-in:{}", i.id),
                    "hardware",
                    st.since as i64,
                    false,
                    say(format!("Flux {} : rien n'arrive ({why})", i.name), format!("Stream {}: nothing arrives ({why})", i.name)),
                    say(format!("Flux {} : reçu de nouveau", i.name), format!("Stream {}: received again", i.name)),
                );
            }
        }
        for o in cfg.outputs.iter().filter(|o| o.enabled) {
            let Some(st) = s.streams.output_status(&o.id) else { continue };
            for d in st.destinations.iter().filter(|d| d.state == St::Error) {
                let label = o
                    .destinations
                    .iter()
                    .find(|x| x.id == d.id)
                    .map(|x| if x.name.is_empty() { x.url.clone() } else { x.name.clone() })
                    .unwrap_or_else(|| d.id.clone());
                let why = d.error.clone().unwrap_or_default();
                push(
                    format!("stream-out:{}:{}", o.id, d.id),
                    "hardware",
                    d.since as i64,
                    false,
                    say(format!("Sortie {} → {label} : n'envoie plus ({why})", o.name), format!("Output {} → {label}: not sending ({why})", o.name)),
                    say(format!("Sortie {} → {label} : envoie de nouveau", o.name), format!("Output {} → {label}: sending again", o.name)),
                );
            }
        }
    }

    if a.clock {
        let cfg = HardwareConfig::load(&s.data_dir);
        let st = s.clock.read();
        let ptp_wanted = cfg.clock.source == hardware::clock::TimeSource::Ptp && !cfg.network.ptp.trim().is_empty();
        if ptp_wanted && !st.ptp.locked {
            let text = if st.ntp.synced == Some(true) {
                say("PTP perdu : l'heure du serveur vient du NTP en attendant".into(), "PTP lost: the server time comes from NTP meanwhile".into())
            } else {
                say("PTP perdu".into(), "PTP lost".into())
            };
            push("ptp".into(), "clock", now, false, text, say("PTP de nouveau verrouillé".into(), "PTP locked again".into()));
        } else if st.ntp.synced == Some(false) {
            push(
                "time".into(),
                "clock",
                now,
                false,
                say("L'heure du serveur n'est plus synchronisée".into(), "The server time is no longer synchronised".into()),
                say("L'heure du serveur est de nouveau synchronisée".into(), "The server time is synchronised again".into()),
            );
        }
    }

    if a.restart && crate::health::crashed_before() && !*s.alerts.restart_told.lock().unwrap() {
        push(
            "restart".into(),
            "restart",
            now,
            true,
            say(
                "Strata Core a redémarré après un arrêt inattendu (voir Santé › Journal)".into(),
                "Strata Core started again after an unexpected stop (see Health › Journal)".into(),
            ),
            String::new(),
        );
    }

    // Les alertes des applis (celles d'une appli désactivée tombent).
    let active_apps: Vec<String> = s.apps.list().into_iter().filter(|x| x.active).map(|x| x.manifest.id).collect();
    let mut declared = s.alerts.declared.lock().unwrap();
    declared.retain(|(app, _), _| active_apps.contains(app));
    for ((app, key), (since, text, over)) in declared.iter() {
        let name = s.apps.get(app).map(|x| x.manifest.name).unwrap_or_else(|| app.clone());
        push(
            format!("declared:{app}:{key}"),
            "app",
            *since,
            false,
            format!("{name} : {text}"),
            if over.is_empty() { String::new() } else { format!("{name} : {over}") },
        );
    }
    out
}

/// Un tour : ce qui va mal, ce qui est fini, et les messages à envoyer.
fn round(s: &AppState) -> Vec<(&'static str, String, String)> {
    let a = s.alerts.settings.load_full();
    let mut seen = look(s);
    let now = now_ms();
    let delay = a.delay_secs as i64 * 1000;
    let mut active = s.alerts.active.lock().unwrap();
    let mut out = Vec::new();
    for p in seen.iter_mut() {
        if let Some(before) = active.iter().find(|b| b.key == p.key) {
            p.since = p.since.min(before.since);
            p.told = before.told;
        }
        if a.enabled && !p.told && (p.once || now - p.since >= delay) {
            p.told = true;
            out.push(("problem", p.key.clone(), p.text.clone()));
            if p.key == "restart" {
                *s.alerts.restart_told.lock().unwrap() = true;
            }
        }
    }
    // Fini : dit s'il avait été dit.
    for b in active.iter().filter(|b| !seen.iter().any(|p| p.key == b.key)) {
        if a.enabled && b.told && !b.over.is_empty() {
            out.push(("ok", b.key.clone(), b.over.clone()));
        }
    }
    // Désactivées : rien n'est gardé pour plus tard.
    if !a.enabled {
        seen.iter_mut().for_each(|p| p.told = false);
    }
    *active = seen;
    out
}

/// Regarde toutes les 5 s tant que le Core tourne.
pub fn start(s: &Shared) {
    let weak = Arc::downgrade(s);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let Some(s) = weak.upgrade() else { return };
            let s2 = s.clone();
            let Ok(out) = tokio::task::spawn_blocking(move || round(&s2)).await else { continue };
            if out.is_empty() {
                continue;
            }
            let s2 = s.clone();
            tokio::task::spawn_blocking(move || {
                let a = s2.alerts.settings.load_full();
                let server = s2.config.load().name.clone();
                for (level, key, text) in out {
                    let mut results = send(&a, &server, level, &text);
                    if a.snmp_traps && crate::snmp::send_trap(&s2, level, &key, &text) {
                        results.push(("snmp".into(), None));
                    }
                    for (channel, r) in &results {
                        if let Some(e) = r {
                            tracing::warn!("alert by {channel}: {e}");
                        }
                    }
                    let mut recent = s2.alerts.recent.lock().unwrap();
                    if recent.len() == 50 {
                        recent.pop_back();
                    }
                    recent.push_front(Sent { at: now_ms(), level, text, results });
                }
            });
        }
    });
}

// ---------- Envoyer ----------

/// Envoie un message par chaque canal réglé : (canal, erreur éventuelle).
fn send(a: &AlertSettings, server: &str, level: &'static str, text: &str) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    if a.webhook_enabled && !a.webhook.is_empty() {
        out.push(("webhook".into(), webhook(a, server, level, text).err()));
    }
    if a.email_enabled && !a.email_to.is_empty() {
        out.push(("email".into(), email(a, server, text).err()));
    }
    out
}

fn webhook(a: &AlertSettings, server: &str, level: &str, text: &str) -> Result<(), String> {
    let mark = match level {
        "problem" => "⚠️",
        "ok" => "✅",
        _ => "ℹ️",
    };
    let line = format!("{mark} [{server}] {text}");
    let body = json!({ "text": line, "content": line, "level": level, "server": server, "message": text, "at": jiff::Timestamp::now().to_string() });
    let r = crate::install::agent().post(&a.webhook).header("Content-Type", "application/json").send(body.to_string()).map_err(|e| e.to_string())?;
    if r.status().is_success() {
        Ok(())
    } else {
        Err(format!("the webhook answered HTTP {}", r.status().as_u16()))
    }
}

// Un petit client SMTP (STARTTLS ou TLS, AUTH PLAIN), sur le rustls du Core. Repris de Cut.

trait Wire: Read + Write {}
impl<T: Read + Write> Wire for T {}

struct Smtp {
    io: Box<dyn Wire>,
}

impl Smtp {
    /// Une réponse (le code de sa dernière ligne), sur plusieurs lignes peut-être (« 250-… »).
    fn reply(&mut self) -> Result<(u16, String), String> {
        let mut all = String::new();
        loop {
            let mut line = Vec::new();
            let mut b = [0u8; 1];
            loop {
                let n = self.io.read(&mut b).map_err(|e| e.to_string())?;
                if n == 0 {
                    return Err("the mail server closed the connection".into());
                }
                line.push(b[0]);
                if b[0] == b'\n' || line.len() > 4096 {
                    break;
                }
            }
            let line = String::from_utf8_lossy(&line).trim_end().to_string();
            all.push_str(&line);
            all.push('\n');
            if line.len() < 4 || line.as_bytes()[3] != b'-' {
                let code = line.get(..3).and_then(|c| c.parse().ok()).ok_or_else(|| format!("unexpected answer: {line}"))?;
                return Ok((code, all));
            }
        }
    }

    fn cmd(&mut self, line: &str, want: &[u16]) -> Result<String, String> {
        self.io.write_all(format!("{line}\r\n").as_bytes()).map_err(|e| e.to_string())?;
        self.expect(want, line.split(' ').next().unwrap_or(line))
    }

    fn expect(&mut self, want: &[u16], what: &str) -> Result<String, String> {
        let (code, text) = self.reply()?;
        if want.contains(&code) {
            Ok(text)
        } else {
            Err(format!("{what}: {}", text.trim()))
        }
    }
}

fn tls(host: &str, tcp: TcpStream) -> Result<Box<dyn Wire>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = rustls_platform_verifier::Verifier::new(provider.clone()).map_err(|e| e.to_string())?;
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|e| e.to_string())?;
    let conn = rustls::ClientConnection::new(Arc::new(config), name).map_err(|e| e.to_string())?;
    Ok(Box::new(rustls::StreamOwned::new(conn, tcp)))
}

/// « =?UTF-8?B?…?= » : un sujet avec des accents.
fn encoded(s: &str) -> String {
    use base64::Engine;
    if s.is_ascii() {
        s.to_string()
    } else {
        format!("=?UTF-8?B?{}?=", base64::engine::general_purpose::STANDARD.encode(s))
    }
}

fn email(a: &AlertSettings, server: &str, text: &str) -> Result<(), String> {
    use base64::Engine;
    let to = recipients(&a.email_to);
    let addr = format!("{}:{}", a.smtp_host, a.smtp_port);
    let sock = std::net::ToSocketAddrs::to_socket_addrs(&addr).map_err(|e| format!("{addr}: {e}"))?.next().ok_or_else(|| format!("{addr}: no address"))?;
    let tcp = TcpStream::connect_timeout(&sock, Duration::from_secs(10)).map_err(|e| format!("{addr}: {e}"))?;
    tcp.set_read_timeout(Some(Duration::from_secs(20))).ok();
    tcp.set_write_timeout(Some(Duration::from_secs(20))).ok();
    let me = gethostname::gethostname().to_string_lossy().into_owned();
    let me = if me.is_empty() { "strata-core".to_string() } else { me };
    let clone = || tcp.try_clone().map_err(|e| e.to_string());
    let mut s = if a.smtp_security == "tls" { Smtp { io: tls(&a.smtp_host, clone()?)? } } else { Smtp { io: Box::new(clone()?) } };
    s.expect(&[220], "connection")?;
    let ehlo = s.cmd(&format!("EHLO {me}"), &[250])?;
    if a.smtp_security == "starttls" {
        if !ehlo.to_ascii_uppercase().contains("STARTTLS") {
            return Err("the mail server does not offer STARTTLS".into());
        }
        s.cmd("STARTTLS", &[220])?;
        s = Smtp { io: tls(&a.smtp_host, tcp)? };
        s.cmd(&format!("EHLO {me}"), &[250])?;
    }
    if !a.smtp_user.is_empty() {
        let token = base64::engine::general_purpose::STANDARD.encode(format!("\0{}\0{}", a.smtp_user, a.smtp_password));
        s.cmd(&format!("AUTH PLAIN {token}"), &[235]).map_err(|_| "the mail server refused the user or the password".to_string())?;
    }
    s.cmd(&format!("MAIL FROM:<{}>", a.smtp_from), &[250])?;
    for r in &to {
        s.cmd(&format!("RCPT TO:<{r}>"), &[250, 251])?;
    }
    s.cmd("DATA", &[354])?;
    let subject = format!("[{server}] {text}");
    let date = jiff::fmt::rfc2822::to_string(&jiff::ZonedDateTime::now()).unwrap_or_default();
    let id = format!("<{}.{}@{me}>", now_ms(), std::process::id());
    let body =
        format!("{text}\r\n\r\n{}: {server}\r\n{}", if a.fr() { "Serveur" } else { "Server" }, jiff::ZonedDateTime::now().strftime("%Y-%m-%d %H:%M:%S %Z"));
    // Une ligne qui commence par un point est doublée (la fin de DATA est un point seul).
    let body: String = body.split("\r\n").map(|l| if l.starts_with('.') { format!(".{l}") } else { l.to_string() }).collect::<Vec<_>>().join("\r\n");
    let msg = format!(
        "From: Strata Core <{}>\r\nTo: {}\r\nSubject: {}\r\nDate: {date}\r\nMessage-ID: {id}\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{body}\r\n.\r\n",
        a.smtp_from,
        to.join(", "),
        encoded(&subject)
    );
    s.io.write_all(msg.as_bytes()).map_err(|e| e.to_string())?;
    s.expect(&[250], "message")?;
    let _ = s.cmd("QUIT", &[221]);
    Ok(())
}

// ---------- Les routes ----------

pub fn routes() -> Router<Shared> {
    Router::new().route("/api/health/alerts", get(read).put(write)).route("/api/health/alerts/test", post(test)).route("/api/health/alerts/app", post(declare))
}

/// Les réglages (mot de passe masqué), les problèmes en cours, les derniers messages.
async fn read(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let names: HashMap<String, String> = s.apps.list().into_iter().map(|a| (a.manifest.id, a.manifest.name)).collect();
    Ok(Json(json!({
        "settings": s.alerts.settings.load().masked(),
        "active": s.alerts.active(),
        "recent": s.alerts.recent.lock().unwrap().iter().cloned().collect::<Vec<_>>(),
        "apps": names,
    })))
}

/// Le mot de passe masqué revient : celui gardé, pour le même serveur seulement.
fn unmask(a: &mut AlertSettings, saved: &AlertSettings) -> Result<(), String> {
    if a.smtp_password == MASK {
        if (a.smtp_host.trim(), a.smtp_port, a.smtp_user.trim()) != (saved.smtp_host.trim(), saved.smtp_port, saved.smtp_user.trim()) {
            return Err("give the password again for this server".into());
        }
        a.smtp_password = saved.smtp_password.clone();
    }
    Ok(())
}

async fn write(State(s): State<Shared>, CurrentUser(me): CurrentUser, Json(mut a): Json<AlertSettings>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    unmask(&mut a, &s.alerts.settings.load()).map_err(bad)?;
    a.check().map_err(bad)?;
    strata_core::files::write_json(&AlertSettings::path(&s.data_dir), &a, true).map_err(|e| bad(format!("cannot save the alerts: {e}")))?;
    s.alerts.settings.store(Arc::new(a.clone()));
    tracing::info!("alert settings changed by {}", me.username);
    Ok(Json(json!({ "settings": a.masked() })))
}

/// Un message d'essai, avec les réglages affichés (enregistrés ou non).
async fn test(State(s): State<Shared>, CurrentUser(me): CurrentUser, Json(mut a): Json<AlertSettings>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    unmask(&mut a, &s.alerts.settings.load()).map_err(bad)?;
    a.check().map_err(bad)?;
    let traps = a.snmp_traps && {
        let t = s.snmp_settings.lock().unwrap();
        t.enabled && !t.traps.is_empty()
    };
    if !(a.webhook_enabled && !a.webhook.is_empty()) && !(a.email_enabled && !a.email_to.is_empty()) && !traps {
        return Err(bad("turn on a webhook, an e-mail address or the SNMP traps"));
    }
    let server = s.config.load().name.clone();
    let text = if a.fr() { "Message d’essai des alertes de Strata Core".to_string() } else { "Strata Core alerts: a test message".to_string() };
    let s2 = s.clone();
    let results = blocking(move || {
        let mut r = send(&a, &server, "info", &text);
        if a.snmp_traps && crate::snmp::send_trap(&s2, "info", "test", &text) {
            r.push(("snmp".into(), None));
        }
        r
    })
    .await?;
    Ok(Json(json!({ "results": results })))
}

/// Une appli déclare une alerte (`active: false` : elle est finie). Depuis la machine seulement.
async fn declare(
    State(s): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    MaybeUser(user): MaybeUser,
    Json(d): Json<Declared>,
) -> ApiResult<Json<Value>> {
    if !(guard::this_machine(peer, &headers) || user.is_some_and(|u| u.admin)) {
        return Err((axum::http::StatusCode::FORBIDDEN, "only the apps of this server".into()));
    }
    if !s.apps.get(&d.app).is_some_and(|a| a.active) {
        return Err(bad(format!("no active app: {}", d.app)));
    }
    let key = d.key.trim();
    if key.is_empty() || key.len() > 64 || d.text.len() > 500 || d.over.len() > 500 {
        return Err(bad("an alert has a key (64 characters at most) and a text (500 at most)"));
    }
    let mut declared = s.alerts.declared.lock().unwrap();
    let k = (d.app.clone(), key.to_string());
    if d.active {
        if d.text.trim().is_empty() {
            return Err(bad("an alert has a text"));
        }
        let since = declared.get(&k).map_or(now_ms(), |x| x.0);
        declared.insert(k, (since, d.text.trim().to_string(), d.over.trim().to_string()));
    } else {
        declared.remove(&k);
    }
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_problem_is_told_after_its_delay_then_when_it_is_over() {
        let dir = std::env::temp_dir().join(format!("strata-alerts-{}", strata_core::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Une carte réglée que la machine ne voit pas.
        std::fs::write(dir.join("hardware.json"), r#"{"cards":[{"id":"gone","name":"Plateau","modelName":"DeckLink Quad 2","connectors":[]}]}"#).unwrap();
        let s = AppState::new(dir.clone());
        s.alerts.settings.store(Arc::new(AlertSettings {
            enabled: true,
            delay_secs: 5,
            system: false,
            clock: false,
            restart: false,
            ..AlertSettings::default()
        }));
        assert!(round(&s).is_empty(), "not yet: it has not lasted");
        let active = s.alerts.active();
        assert_eq!(active[0].key, "card:gone");
        assert!(active[0].text.contains("« Plateau » (DeckLink Quad 2) plus vue"));
        // Il dure.
        s.alerts.active.lock().unwrap()[0].since -= 10_000;
        let out = round(&s);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "problem");
        assert!(round(&s).is_empty(), "told once");
        // Fini.
        std::fs::write(dir.join("hardware.json"), "{}").unwrap();
        let out = round(&s);
        assert_eq!((out[0].0, out[0].1.as_str(), out[0].2.as_str()), ("ok", "card:gone", "Carte DeckLink « Plateau » (DeckLink Quad 2) de nouveau vue"));
        assert!(s.alerts.active().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checks() {
        let mut a = AlertSettings::default();
        assert!(a.check().is_ok());
        a.email_to = "ops@example.com, regie@example.com".into();
        assert!(a.check().is_err());
        a.smtp_host = "smtp.example.com".into();
        a.smtp_from = "strata@example.com".into();
        assert!(a.check().is_ok());
        assert_eq!(recipients(&a.email_to), ["ops@example.com", "regie@example.com"]);
        a.email_to = "x@y.z>\r\nRCPT TO:<evil@example.com".into();
        assert!(a.check().is_err());
        a.email_to.clear();
        a.webhook = "ftp://x".into();
        assert!(a.check().is_err());
        assert_eq!(encoded("plain"), "plain");
        assert!(encoded("Entrée coupée").starts_with("=?UTF-8?B?"));
    }
}
