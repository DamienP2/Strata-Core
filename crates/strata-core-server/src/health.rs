//! La santé du serveur (Réglages › Santé › Journal), reprise de CG :
//!
//! - **le journal sur disque** : `<données>/health/journal-<jour>.jsonl`, 30 jours, ce que le Core
//!   journalise (INFO et plus grave) et ce qu'écrivent les applis qu'il lance ;
//! - **les plantages** : `<données>/health/crashes/`, 90 jours : une panique du Core, un arrêt qu'il
//!   n'a pas fait lui-même (coupure, arrêt forcé), une appli qui s'arrête seule (avec la fin de son
//!   journal) ;
//! - **les incidents** du dernier jour, une même ligne comptée une fois ;
//! - **l'export** pour le support : tout cela, et les réglages sans leurs secrets.
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use axum::{
    extract::{Query, State},
    http::header,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    api::{blocking, internal, require, ApiResult},
    auth::CurrentUser,
    logs::now_ms,
    state::Shared,
};

/// Jours gardés du journal.
pub const JOURNAL_DAYS: i64 = 30;
/// Jours gardés d'un rapport de plantage.
const CRASH_DAYS: i64 = 90;
const DAY_MS: i64 = 86_400_000;

static DIR: OnceLock<PathBuf> = OnceLock::new();
static STARTED: OnceLock<i64> = OnceLock::new();
/// Le fichier du journal ouvert, et son jour.
static FILE: Mutex<Option<(String, fs::File)>> = Mutex::new(None);
/// Le dernier démarrage suivait un arrêt inattendu.
static CRASHED_BEFORE: OnceLock<bool> = OnceLock::new();

/// Une ligne du journal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Line {
    /// ms depuis 1970.
    pub at: i64,
    /// CRASH, ERROR, WARN ou INFO.
    pub level: String,
    /// `server`, `hardware`, `apps`, `users`, `updates`, ou `app` (une appli : `target` est son id).
    pub cat: String,
    pub target: String,
    pub msg: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fields: String,
}

/// Un plantage.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Crash {
    pub at: i64,
    /// `panic` (le Core), `stop` (le Core arrêté sans le vouloir), `app` (une appli arrêtée seule).
    pub kind: String,
    /// L'appli (vide : le Core).
    #[serde(default)]
    pub app: String,
    pub version: String,
    #[serde(default)]
    pub thread: String,
    pub message: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub backtrace: String,
    /// Depuis combien de temps il tournait.
    #[serde(default)]
    pub uptime_secs: i64,
    /// La fin du journal de l'appli.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub log: Vec<String>,
}

/// La partie du Core d'où vient une ligne.
pub fn category(target: &str) -> &'static str {
    let has = |s: &str| target.contains(s);
    if has("hardware") || has("decklink") || has("ndi") {
        "hardware"
    } else if has("auth") || has("sso") || has("users") || has("groups") {
        "users"
    } else if has("apps") || has("install") || has("relay") || has("supervisor") {
        "apps"
    } else if has("updates") {
        "updates"
    } else {
        "server"
    }
}

fn day_of(ms: i64) -> String {
    jiff::Timestamp::from_millisecond(ms)
        .map(|t| t.to_zoned_datetime(jiff::tz::TimeZone::system()).strftime("%Y-%m-%d").to_string())
        .unwrap_or_else(|_| "unknown".into())
}

fn write_line(line: &Line, wait: bool) {
    let Some(dir) = DIR.get() else { return };
    // Une panique pendant que le fichier est tenu : ne jamais l'attendre (même fil).
    let mut guard = if wait {
        FILE.lock().unwrap_or_else(|e| e.into_inner())
    } else {
        match FILE.try_lock() {
            Ok(g) => g,
            Err(_) => return,
        }
    };
    let day = day_of(line.at);
    if guard.as_ref().is_none_or(|(d, _)| *d != day) {
        let f = fs::OpenOptions::new().create(true).append(true).open(dir.join(format!("journal-{day}.jsonl")));
        *guard = f.ok().map(|f| (day, f));
    }
    if let (Some((_, f)), Ok(s)) = (guard.as_mut(), serde_json::to_string(line)) {
        let _ = writeln!(f, "{s}");
    }
}

/// Garde une ligne du Core (INFO ou plus grave) : appelé par l'anneau (`logs::Ring`).
pub fn record(e: &crate::logs::Entry) {
    if matches!(e.level, "DEBUG" | "TRACE") {
        return;
    }
    write_line(
        &Line {
            at: e.at,
            level: e.level.to_string(),
            cat: category(&e.target).to_string(),
            target: e.target.clone(),
            msg: e.message.clone(),
            fields: e.fields.clone(),
        },
        true,
    );
}

/// Le niveau d'une ligne écrite par une appli, d'après ce qu'elle dit.
fn app_level(text: &str) -> &'static str {
    let head: String = text.chars().take(64).collect::<String>().to_ascii_uppercase();
    if head.contains("ERROR") || head.contains("PANIC") || head.contains("FATAL") {
        "ERROR"
    } else if head.contains("WARN") {
        "WARN"
    } else {
        "INFO"
    }
}

/// Garde une ligne écrite par une appli (sa sortie standard ou d'erreur).
pub fn app_line(app: &str, text: &str) {
    write_line(&Line { at: now_ms(), level: app_level(text).into(), cat: "app".into(), target: app.into(), msg: text.into(), fields: String::new() }, true);
}

fn crashes_dir() -> Option<PathBuf> {
    DIR.get().map(|d| d.join("crashes"))
}

/// Garde-fou des rapports de plantage : (endroit → dernier rapport), et les rapports de l'heure.
type CrashSeen = (HashMap<String, Instant>, Vec<Instant>);

fn crash_allowed(place: &str) -> bool {
    static SEEN: Mutex<Option<CrashSeen>> = Mutex::new(None);
    // Dans le crochet de panique : jamais bloquer (une panique sous ce verrou).
    let Ok(mut g) = SEEN.try_lock() else { return false };
    let (by_place, hour) = g.get_or_insert_with(Default::default);
    let now = Instant::now();
    hour.retain(|t| now.duration_since(*t) < Duration::from_secs(3600));
    if hour.len() >= 20 || by_place.get(place).is_some_and(|t| now.duration_since(*t) < Duration::from_secs(60)) {
        return false;
    }
    hour.push(now);
    by_place.insert(place.to_string(), now);
    true
}

fn crash_file(c: &Crash) -> String {
    if c.app.is_empty() {
        format!("{}-{}.json", c.at, c.kind)
    } else {
        format!("{}-app-{}.json", c.at, c.app)
    }
}

fn save_crash(c: &Crash) {
    let Some(dir) = crashes_dir() else { return };
    let _ = fs::create_dir_all(&dir);
    if let Ok(s) = serde_json::to_string_pretty(c) {
        let _ = fs::write(dir.join(crash_file(c)), s);
    }
    let msg = match c.kind.as_str() {
        "panic" => format!("panic: {} · {}", c.message, c.location),
        "app" => format!("{} stopped by itself ({})", c.app, c.message),
        _ => c.message.clone(),
    };
    let (cat, target) = if c.app.is_empty() { ("server", "crash") } else { ("app", c.app.as_str()) };
    write_line(&Line { at: c.at, level: "CRASH".into(), cat: cat.into(), target: target.into(), msg, fields: String::new() }, false);
}

/// Une appli s'est arrêtée seule : son rapport, avec la fin de son journal.
pub fn app_crash(app: &str, version: &str, how: &str, uptime_secs: i64, log: Vec<String>) {
    save_crash(&Crash {
        at: now_ms(),
        kind: "app".into(),
        app: app.into(),
        version: version.into(),
        thread: String::new(),
        message: how.into(),
        location: String::new(),
        backtrace: String::new(),
        uptime_secs,
        log,
    });
}

/// Les rapports gardés, les plus récents d'abord.
pub fn crashes() -> Vec<Crash> {
    let Some(dir) = crashes_dir() else { return Vec::new() };
    let mut out: Vec<Crash> =
        fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| serde_json::from_str(&fs::read_to_string(e.path()).ok()?).ok()).collect();
    out.sort_by_key(|c| std::cmp::Reverse(c.at));
    out
}

/// Le Core a démarré après un arrêt qu'il n'avait pas fait lui-même.
pub fn crashed_before() -> bool {
    CRASHED_BEFORE.get().copied().unwrap_or(false)
}

pub fn uptime_secs() -> i64 {
    STARTED.get().map_or(0, |s| (now_ms() - s) / 1000)
}

/// La marque d'un Core en marche : retirée par un arrêt qu'il fait lui-même.
#[derive(Serialize, Deserialize)]
struct Running {
    started: i64,
    /// Dernier signe de vie.
    beat: i64,
    #[serde(default)]
    pid: u32,
}

/// Un autre processus `pid` tourne.
fn alive(pid: u32) -> bool {
    if pid == 0 || pid == std::process::id() {
        return false;
    }
    // SAFETY: le signal 0 ne fait que vérifier que le processus existe.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

fn marker() -> Option<PathBuf> {
    DIR.get().map(|d| d.join("running.json"))
}

fn beat() {
    if let (Some(m), Some(started)) = (marker(), STARTED.get()) {
        let r = Running { started: *started, beat: now_ms(), pid: std::process::id() };
        if let Ok(s) = serde_json::to_string(&r) {
            let _ = fs::write(m, s);
        }
    }
}

/// Retire ce qui a passé son temps.
fn purge() {
    let Some(dir) = DIR.get() else { return };
    let now = now_ms();
    let old_day = day_of(now - JOURNAL_DAYS * DAY_MS);
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(day) = name.strip_prefix("journal-").and_then(|n| n.strip_suffix(".jsonl")) {
            if day < old_day.as_str() {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    if let Some(d) = crashes_dir() {
        for c in crashes().into_iter().filter(|c| c.at < now - CRASH_DAYS * DAY_MS) {
            let _ = fs::remove_file(d.join(crash_file(&c)));
        }
    }
}

/// Ouvre le journal dans `<données>/health` : note un arrêt que le Core précédent n'a pas fait
/// lui-même, garde ce qui a été journalisé jusque-là, surveille les paniques.
pub fn start(data_dir: &Path) {
    let dir = data_dir.join("health");
    let _ = fs::create_dir_all(dir.join("crashes"));
    if DIR.set(dir).is_err() {
        return;
    }
    let started = now_ms();
    let _ = STARTED.set(started);
    // Le Core précédent ne s'est pas arrêté lui-même, et aucune panique ne dit pourquoi. Un Core
    // encore en train de s'arrêter (redémarrage, mise à jour) est attendu : il retire sa marque.
    let read = || marker().and_then(|m| fs::read_to_string(m).ok()).and_then(|s| serde_json::from_str::<Running>(&s).ok());
    let mut last = read();
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while let Some(prev) = &last {
        if !alive(prev.pid) || std::time::Instant::now() > until {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
        last = read();
    }
    let mut crashed = false;
    if let Some(prev) = last.filter(|p| !alive(p.pid)) {
        crashed = true;
        if !crashes().iter().any(|c| c.at >= prev.started && c.app.is_empty()) {
            save_crash(&Crash {
                at: prev.beat,
                kind: "stop".into(),
                app: String::new(),
                version: String::new(),
                thread: String::new(),
                message: "unexpected stop (power cut, forced stop or crash)".into(),
                location: String::new(),
                backtrace: String::new(),
                uptime_secs: (prev.beat - prev.started) / 1000,
                log: Vec::new(),
            });
        }
    }
    let _ = CRASHED_BEFORE.set(crashed);
    beat();
    // Ce qui a été journalisé avant que le journal ait son dossier.
    for e in crate::logs::since(0, Some("info"), 5000).0 {
        record(&e);
    }
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Au plus un rapport par endroit et par minute, et 20 par heure : une panique répétée
        // (requête en boucle) ne remplit pas le disque.
        let place = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        if !crash_allowed(&place) {
            prev(info);
            return;
        }
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic".into());
        save_crash(&Crash {
            at: now_ms(),
            kind: "panic".into(),
            app: String::new(),
            version: strata_core::VERSION.into(),
            thread: std::thread::current().name().unwrap_or("?").to_string(),
            message,
            location: info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default(),
            backtrace: std::backtrace::Backtrace::force_capture().to_string(),
            uptime_secs: uptime_secs(),
            log: Vec::new(),
        });
        prev(info);
    }));
    purge();
    std::thread::Builder::new()
        .name("health".into())
        .spawn(|| {
            let mut n = 0u32;
            loop {
                std::thread::sleep(Duration::from_secs(30));
                beat();
                n += 1;
                if n.is_multiple_of(720) {
                    purge();
                }
            }
        })
        .ok();
}

/// Un arrêt que le Core fait lui-même (arrêt, redémarrage, mise à jour) : pas un plantage.
pub fn stopped() {
    if let Some(m) = marker() {
        let _ = fs::remove_file(m);
    }
}

// ---------- Lire ----------

/// Les lignes des `days` derniers jours, les plus récentes d'abord, avant `before`.
pub fn read_journal(days: i64, before: Option<i64>, keep: impl Fn(&Line) -> bool, limit: usize) -> Vec<Line> {
    let Some(dir) = DIR.get() else { return Vec::new() };
    let from = now_ms() - days * DAY_MS;
    let mut files: Vec<(String, PathBuf)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            Some((n.strip_prefix("journal-")?.strip_suffix(".jsonl")?.to_string(), e.path()))
        })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    let first_day = day_of(from);
    let mut out = Vec::new();
    for (day, path) in files {
        if day < first_day {
            break;
        }
        let Ok(text) = fs::read_to_string(path) else { continue };
        let mut lines: Vec<Line> =
            text.lines().filter_map(|l| serde_json::from_str::<Line>(l).ok()).filter(|l| l.at >= from && before.is_none_or(|b| l.at < b) && keep(l)).collect();
        lines.reverse();
        out.extend(lines);
        if out.len() >= limit {
            out.truncate(limit);
            break;
        }
    }
    out
}

fn rank(level: &str) -> u8 {
    match level {
        "CRASH" => 0,
        "ERROR" => 1,
        "WARN" => 2,
        _ => 3,
    }
}

#[derive(Deserialize)]
struct JournalQuery {
    /// crash, error, warn ou info : ce niveau et plus grave.
    #[serde(default)]
    level: Option<String>,
    #[serde(default)]
    days: Option<i64>,
    /// `server`, `hardware`… ou `app:<id>`.
    #[serde(default)]
    cat: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    before: Option<i64>,
    #[serde(default)]
    limit: Option<usize>,
}

async fn journal(CurrentUser(me): CurrentUser, Query(q): Query<JournalQuery>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let max = rank(&q.level.as_deref().unwrap_or("info").to_ascii_uppercase());
    let days = q.days.unwrap_or(1).clamp(1, JOURNAL_DAYS);
    let limit = q.limit.unwrap_or(300).min(2000);
    let needle = q.q.unwrap_or_default().to_lowercase();
    let cat = q.cat.filter(|c| !c.is_empty() && c != "all");
    let lines = blocking(move || {
        read_journal(
            days,
            q.before,
            |l| {
                let in_cat = match cat.as_deref() {
                    None => true,
                    Some(c) => match c.strip_prefix("app:") {
                        Some(id) => l.cat == "app" && l.target == id,
                        None => l.cat == c,
                    },
                };
                rank(&l.level) <= max
                    && in_cat
                    && (needle.is_empty() || l.msg.to_lowercase().contains(&needle) || l.fields.to_lowercase().contains(&needle) || l.target.contains(&needle))
            },
            limit + 1,
        )
    })
    .await?;
    let more = lines.len() > limit;
    Ok(Json(json!({ "lines": &lines[..lines.len().min(limit)], "more": more })))
}

/// Jusqu'à quand les plantages ont été vus.
fn seen_at() -> i64 {
    DIR.get()
        .and_then(|d| fs::read_to_string(d.join("seen.json")).ok())
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.get("crashesSeenAt").and_then(|x| x.as_i64()))
        .unwrap_or(0)
}

/// Un message sans ses parties changeantes (nombres) : les lignes qui disent la même chose sont
/// un seul incident.
fn shape(msg: &str) -> String {
    let mut out = String::new();
    let mut digits = false;
    for c in msg.chars() {
        if c.is_ascii_digit() {
            if !digits {
                out.push('#');
            }
            digits = true;
        } else {
            digits = false;
            out.push(c);
        }
    }
    out
}

/// Niveau, partie, appli, forme du message.
type GroupKey = (String, String, String, String);
/// Combien, première et dernière fois, la dernière ligne.
type Group = (usize, i64, i64, Line);

/// Ce que la page montre d'abord : plantages, erreurs, incidents du dernier jour.
async fn summary(CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let v = blocking(|| {
        let now = now_ms();
        let crashes = crashes();
        let month: Vec<&Crash> = crashes.iter().filter(|c| c.at >= now - JOURNAL_DAYS * DAY_MS).collect();
        let day = read_journal(1, None, |l| rank(&l.level) <= 2 && l.level != "CRASH" && l.at >= now - DAY_MS, 100_000);
        let errors = day.iter().filter(|l| l.level == "ERROR").count();
        let warnings = day.iter().filter(|l| l.level == "WARN").count();
        let mut groups: BTreeMap<GroupKey, Group> = BTreeMap::new();
        for l in &day {
            let key = (l.level.clone(), l.cat.clone(), if l.cat == "app" { l.target.clone() } else { String::new() }, shape(&l.msg));
            let g = groups.entry(key).or_insert((0, l.at, l.at, l.clone()));
            g.0 += 1;
            g.1 = g.1.min(l.at);
            if l.at >= g.2 {
                g.2 = l.at;
                g.3 = l.clone();
            }
        }
        let mut incidents: Vec<Value> = month
            .iter()
            .map(|c| json!({ "level": "CRASH", "cat": if c.app.is_empty() { "server" } else { "app" }, "app": c.app, "kind": c.kind, "title": c.message, "detail": c.location, "first": c.at, "last": c.at, "count": 1, "crash": c }))
            .collect();
        let mut rest: Vec<_> = groups.into_values().collect();
        rest.sort_by(|a, b| rank(&a.3.level).cmp(&rank(&b.3.level)).then(b.2.cmp(&a.2)));
        incidents.extend(rest.into_iter().take(40).map(|(n, first, last, l)| {
            json!({ "level": l.level, "cat": l.cat, "app": if l.cat == "app" { l.target.as_str() } else { "" }, "title": l.msg, "detail": l.fields, "first": first, "last": last, "count": n })
        }));
        let seen = seen_at();
        json!({
            "version": strata_core::VERSION,
            "startedAt": STARTED.get(),
            "uptimeSecs": uptime_secs(),
            "crashes": month.len(),
            "lastCrash": month.first().map(|c| c.at),
            "unseenCrash": month.iter().any(|c| c.at > seen),
            "errors": errors,
            "warnings": warnings,
            "incidents": incidents,
        })
    })
    .await?;
    Ok(Json(v))
}

/// Les plantages jusqu'ici ont été vus.
async fn seen(CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    if let Some(d) = DIR.get() {
        fs::write(d.join("seen.json"), json!({ "crashesSeenAt": now_ms() }).to_string()).map_err(internal)?;
    }
    Ok(Json(json!({ "ok": true })))
}

// ---------- L'export pour le support ----------

/// Les réglages sans leurs secrets (mots de passe, clés, jetons).
pub fn redact(v: &mut Value) {
    match v {
        Value::Object(o) => {
            for (k, x) in o.iter_mut() {
                let k = k.to_ascii_lowercase();
                if ["password", "secret", "token", "apikey", "api_key", "key", "pass", "hash", "webhook", "url"].iter().any(|s| k == *s || k.ends_with(s))
                    && x.is_string()
                {
                    *x = Value::String("•••".into());
                } else {
                    redact(x);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(redact),
        _ => {}
    }
}

fn add(t: &mut tar::Builder<flate2::write::GzEncoder<Vec<u8>>>, name: &str, bytes: &[u8]) -> Result<(), String> {
    let mut h = tar::Header::new_gnu();
    h.set_size(bytes.len() as u64);
    h.set_mode(0o644);
    h.set_mtime((now_ms() / 1000) as u64);
    h.set_cksum();
    t.append_data(&mut h, name, bytes).map_err(|e| e.to_string())
}

pub fn archive(data_dir: &Path, system: &str) -> Result<Vec<u8>, String> {
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut t = tar::Builder::new(gz);
    add(&mut t, "system.txt", system.as_bytes())?;
    if let Some(dir) = DIR.get() {
        for e in fs::read_dir(dir).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with("journal-") {
                if let Ok(b) = fs::read(e.path()) {
                    add(&mut t, &format!("journal/{n}"), &b)?;
                }
            }
        }
    }
    for c in crashes() {
        add(&mut t, &format!("crashes/{}", crash_file(&c)), serde_json::to_string_pretty(&c).unwrap_or_default().as_bytes())?;
    }
    // Les réglages, sans secrets (pas les comptes ni les sessions).
    for f in ["config.json", "hardware.json", "apps.json", "groups.json", "alerts.json"] {
        if let Some(mut v) = fs::read_to_string(data_dir.join(f)).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
            redact(&mut v);
            add(&mut t, &format!("settings/{f}"), serde_json::to_string_pretty(&v).unwrap_or_default().as_bytes())?;
        }
    }
    let gz = t.into_inner().map_err(|e| e.to_string())?;
    gz.finish().map_err(|e| e.to_string())
}

/// Tout ce dont le support a besoin.
async fn export(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<impl IntoResponse> {
    require(me.admin)?;
    let s2 = s.clone();
    let bytes = blocking(move || {
        let system = crate::resources::system_text(&s2);
        archive(&s2.data_dir, &system)
    })
    .await?
    .map_err(internal)?;
    let name = format!("strata-health-{}.tar.gz", day_of(now_ms()));
    Ok(([(header::CONTENT_TYPE, "application/gzip".to_string()), (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\""))], bytes))
}

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/health/summary", get(summary))
        .route("/api/health/journal", get(journal))
        .route("/api/health/seen", post(seen))
        .route("/api/health/export", get(export))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_an_unexpected_stop_and_an_app_crash_are_kept() {
        let dir = std::env::temp_dir().join(format!("strata-health-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        // Le Core précédent a laissé sa marque : il ne s'est pas arrêté lui-même.
        fs::create_dir_all(dir.join("health")).unwrap();
        let beat_at = now_ms() - 60_000;
        fs::write(dir.join("health/running.json"), json!({ "started": beat_at - 60_000, "beat": beat_at }).to_string()).unwrap();
        start(&dir);
        assert!(crashed_before());
        let r = std::panic::catch_unwind(|| panic!("boom in a test"));
        assert!(r.is_err());
        app_crash("livecut", "0.3.0", "code 3", 12, vec!["ERROR no input".into()]);
        app_line("livecut", "2026-11-27T12:00:00Z ERROR livecut: cannot open the input");
        let c = crashes();
        assert!(c.iter().any(|c| c.kind == "panic" && c.message == "boom in a test" && c.location.contains("health.rs")));
        assert!(c.iter().any(|c| c.kind == "stop" && c.at == beat_at && c.uptime_secs == 60));
        assert!(c.iter().any(|c| c.kind == "app" && c.app == "livecut" && c.log == ["ERROR no input"]));
        let lines = read_journal(JOURNAL_DAYS, None, |l| l.level == "CRASH", 10);
        assert!(lines.iter().any(|l| l.msg.contains("boom in a test")));
        assert!(lines.iter().any(|l| l.msg == "livecut stopped by itself (code 3)"));
        let app = read_journal(1, None, |l| l.cat == "app" && l.level == "ERROR", 10);
        assert_eq!(app[0].target, "livecut");
        stopped();
        assert!(!dir.join("health/running.json").exists());
        let bytes = archive(&dir, "test").unwrap();
        assert!(bytes.len() > 100);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lines_are_grouped_and_secrets_hidden() {
        assert_eq!(shape("NDI: 24 frames dropped in 10 s"), "NDI: # frames dropped in # s");
        assert_eq!(category("strata_core_server::hardware::capture"), "hardware");
        assert_eq!(category("strata_core_server::supervisor"), "apps");
        assert_eq!(category("strata_core_server::tls"), "server");
        assert_eq!(app_level("2026-11-27 WARN low disk"), "WARN");
        assert_eq!(app_level("listening on 127.0.0.1:8101"), "INFO");
        let mut v = json!({ "smtp": { "smtpPassword": "x", "host": "h" }, "sso": {"clientSecret": "s"}, "keyer": 2 });
        redact(&mut v);
        assert_eq!(v["smtp"]["smtpPassword"], "•••");
        assert_eq!(v["smtp"]["host"], "h");
        assert_eq!(v["sso"]["clientSecret"], "•••");
        assert_eq!(v["keyer"], 2);
    }
}
