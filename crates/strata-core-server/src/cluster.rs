//! Plusieurs serveurs, un seul cockpit (brief § 4 bis).
//!
//! - **Identité** : chaque serveur a une clé Ed25519 (`<données>/server-key.pk8`, privée) ; son
//!   identifiant est tiré de sa clé publique.
//! - **Rattacher** : le maître donne un code d'invitation (une fois, 10 minutes) ; le membre l'envoie
//!   avec sa clé publique, le maître répond avec la sienne. Les deux messages sont scellés par le code
//!   (HMAC) : sans lui, personne ne s'interpose. Ensuite, chaque requête entre eux est signée.
//! - **Battement** : le membre ouvre la connexion vers le maître (jamais l'inverse) toutes les 3 s :
//!   son nom, ses applis actives et leur état, ses problèmes en cours. Le maître répond avec ses
//!   groupes. Sans battement depuis 10 s, le membre est injoignable.
//! - **Connexion unique** : sans session, un membre renvoie au maître, qui renvoie aussitôt avec un
//!   jeton signé (identité, groupes, 60 s, une fois). Le maître tombé, le membre garde ses comptes
//!   locaux (secours).
//! - **Détacher** : depuis le maître ou depuis le membre ; le membre redevient autonome.
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};

use axum::{
    body::Bytes,
    extract::{Path as UrlPath, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{delete, get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use ring::{
    hmac,
    rand::{SecureRandom, SystemRandom},
    signature::{self, Ed25519KeyPair, KeyPair},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    api::{bad, blocking, require, ApiResult},
    auth::{self, CurrentUser, MaybeUser, User},
    logs::now_ms,
    sso::enc,
    state::{AppState, Shared},
};

/// Validité d'un code d'invitation.
const INVITE_TTL: Duration = Duration::from_secs(600);
/// Un membre sans battement depuis ce temps est injoignable.
const UP_FOR: Duration = Duration::from_secs(10);
const BEAT_EVERY: Duration = Duration::from_secs(3);
/// Écart d'horloge toléré sur une requête signée, et durée d'un jeton de connexion.
const SKEW_MS: i64 = 60_000;
const TOKEN_MS: i64 = 60_000;
const STATE_COOKIE: &str = "strata_cluster_state";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub id: String,
    pub name: String,
    /// L'adresse de son Core (`https://strata02.regie.fr/`).
    pub url: String,
    /// Sa clé publique (base64url).
    pub key: String,
    pub joined: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MasterRef {
    pub id: String,
    pub name: String,
    pub url: String,
    pub key: String,
    /// L'autorité du maître (HTTPS fait par Strata), pour vérifier son certificat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca: Option<String>,
    pub joined: i64,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
struct ClusterFile {
    members: Vec<Member>,
    master: Option<MasterRef>,
}

/// Le dernier battement d'un membre (côté maître).
#[derive(Clone)]
struct Live {
    at: Instant,
    beat: Beat,
}

/// Ce qu'un membre dit de lui toutes les 3 s.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Beat {
    pub name: String,
    pub version: String,
    pub apps: Vec<BeatApp>,
    pub problems: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct BeatApp {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub url: String,
    pub up: bool,
}

pub struct Cluster {
    path: PathBuf,
    key: Ed25519KeyPair,
    /// L'identifiant de ce serveur (tiré de sa clé publique).
    pub id: String,
    file: Mutex<ClusterFile>,
    invites: Mutex<Vec<(String, Instant)>>,
    live: Mutex<HashMap<String, Live>>,
    /// Côté membre : le dernier battement réussi, la dernière erreur.
    master_ok: Mutex<Option<Instant>>,
    master_error: Mutex<Option<String>>,
    /// Les jetons de connexion déjà servis (une fois chacun).
    used: Mutex<HashMap<String, i64>>,
    /// Les signatures de requêtes déjà reçues (pas de rejeu pendant leur minute de validité).
    seen: Mutex<HashMap<String, i64>>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn sha256(b: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, b).as_ref())
}

/// Un code d'invitation lisible : 10 caractères sans ambiguïté, « ABCD-EFGH-JK ».
fn new_code() -> String {
    const ALPHA: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut b = [0u8; 10];
    let _ = SystemRandom::new().fill(&mut b);
    let s: String = b.iter().map(|x| ALPHA[(*x as usize) % ALPHA.len()] as char).collect();
    format!("{}-{}-{}", &s[..4], &s[4..8], &s[8..])
}

fn clean_code(c: &str) -> String {
    c.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect()
}

fn mac(code: &str, body: &[u8]) -> String {
    let k = hmac::Key::new(hmac::HMAC_SHA256, clean_code(code).as_bytes());
    hex(hmac::sign(&k, body).as_ref())
}

fn mac_ok(code: &str, body: &[u8], given: &str) -> bool {
    let k = hmac::Key::new(hmac::HMAC_SHA256, clean_code(code).as_bytes());
    let Some(tag) = (0..given.len() / 2).map(|i| u8::from_str_radix(given.get(i * 2..i * 2 + 2)?, 16).ok()).collect::<Option<Vec<u8>>>() else { return false };
    hmac::verify(&k, body, &tag).is_ok()
}

fn verify(key_b64: &str, msg: &[u8], sig_b64: &str) -> bool {
    let (Ok(key), Ok(sig)) = (B64.decode(key_b64), B64.decode(sig_b64)) else { return false };
    signature::UnparsedPublicKey::new(&signature::ED25519, key).verify(msg, &sig).is_ok()
}

/// Ce qu'on signe d'une requête : méthode, chemin, date, empreinte du corps.
fn request_msg(method: &str, path: &str, date: i64, body: &[u8]) -> Vec<u8> {
    format!("{method}\n{path}\n{date}\n{}", sha256(body)).into_bytes()
}

fn response_msg(date: i64, body: &[u8]) -> Vec<u8> {
    format!("RESPONSE\n{date}\n{}", sha256(body)).into_bytes()
}

impl Cluster {
    pub fn load(data_dir: &Path) -> Self {
        let key_path = data_dir.join("server-key.pk8");
        let key = std::fs::read(&key_path).ok().and_then(|b| Ed25519KeyPair::from_pkcs8(&b).ok()).unwrap_or_else(|| {
            let pk8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).expect("a server key");
            let _ = std::fs::create_dir_all(data_dir);
            if let Err(e) = strata_core::files::write_atomic(&key_path, pk8.as_ref(), true) {
                tracing::warn!("cannot keep the server key: {e}");
            }
            Ed25519KeyPair::from_pkcs8(pk8.as_ref()).expect("a server key")
        });
        let id = sha256(key.public_key().as_ref())[..12].to_string();
        let path = data_dir.join("cluster.json");
        let file = strata_core::files::read_json(&path).unwrap_or_default();
        Cluster {
            path,
            key,
            id,
            file: Mutex::new(file),
            invites: Mutex::default(),
            live: Mutex::default(),
            master_ok: Mutex::default(),
            master_error: Mutex::default(),
            used: Mutex::default(),
            seen: Mutex::default(),
        }
    }

    pub fn public_key(&self) -> String {
        B64.encode(self.key.public_key().as_ref())
    }

    fn sign(&self, msg: &[u8]) -> String {
        B64.encode(self.key.sign(msg).as_ref())
    }

    fn save(&self, f: &ClusterFile) -> Result<(), String> {
        strata_core::files::write_json(&self.path, f, false).map_err(|e| format!("cannot save the servers: {e}"))
    }

    pub fn master(&self) -> Option<MasterRef> {
        self.file.lock().unwrap().master.clone()
    }

    pub fn members(&self) -> Vec<Member> {
        self.file.lock().unwrap().members.clone()
    }

    /// Côté membre : le maître a répondu il y a moins de 10 s.
    pub fn master_up(&self) -> bool {
        self.master_ok.lock().unwrap().is_some_and(|t| t.elapsed() < UP_FOR)
    }

    /// Côté maître : les membres joignables, avec leur dernier battement.
    pub fn live_members(&self) -> Vec<(Member, Option<Beat>)> {
        let live = self.live.lock().unwrap();
        self.members()
            .into_iter()
            .map(|m| {
                let b = live.get(&m.id).filter(|l| l.at.elapsed() < UP_FOR).map(|l| l.beat.clone());
                (m, b)
            })
            .collect()
    }

    /// Vérifie une requête signée par un membre : son identifiant.
    fn check_member(&self, method: &str, path: &str, headers: &HeaderMap, body: &[u8]) -> Result<Member, (StatusCode, String)> {
        let get = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        let (id, date, sig) = (get("x-strata-server"), get("x-strata-date").parse::<i64>().unwrap_or(0), get("x-strata-signature"));
        if (now_ms() - date).abs() > SKEW_MS {
            return Err((StatusCode::UNAUTHORIZED, "the clocks of the two servers differ by more than a minute".into()));
        }
        let Some(m) = self.members().into_iter().find(|m| m.id == id) else {
            return Err((StatusCode::GONE, "unknown server".into()));
        };
        if !verify(&m.key, &request_msg(method, path, date, body), &sig) {
            return Err((StatusCode::UNAUTHORIZED, "bad signature".into()));
        }
        let mut seen = self.seen.lock().unwrap();
        let now = now_ms();
        seen.retain(|_, at| now - *at <= 2 * SKEW_MS);
        if seen.insert(sig, now).is_some() {
            return Err((StatusCode::UNAUTHORIZED, "this request was already received".into()));
        }
        Ok(m)
    }

    /// Une réponse signée par ce serveur.
    fn signed(&self, status: StatusCode, v: Value) -> Response {
        let body = v.to_string().into_bytes();
        let date = now_ms();
        let sig = self.sign(&response_msg(date, &body));
        let mut res = (status, [(header::CONTENT_TYPE, "application/json")], body).into_response();
        res.headers_mut().insert("x-strata-date", HeaderValue::from(date));
        if let Ok(v) = HeaderValue::from_str(&sig) {
            res.headers_mut().insert("x-strata-signature", v);
        }
        res
    }
}

// ---------- Les appels vers le maître (côté membre) ----------

fn agent(insecure: bool, ca: Option<&str>) -> ureq::Agent {
    let mut tls = ureq::tls::TlsConfig::builder();
    if insecure {
        // Le rattachement : le code scelle les deux messages, le certificat n'est pas encore connu.
        tls = tls.disable_verification(true);
    } else if let Some(pem) = ca {
        let certs: Vec<ureq::tls::Certificate<'static>> = ureq::tls::parse_pem(pem.as_bytes())
            .filter_map(|p| match p {
                Ok(ureq::tls::PemItem::Certificate(c)) => Some(c),
                _ => None,
            })
            .collect();
        tls = tls.root_certs(ureq::tls::RootCerts::new_with_certs(&certs));
    } else {
        tls = tls.root_certs(ureq::tls::RootCerts::PlatformVerifier);
    }
    ureq::Agent::config_builder()
        .tls_config(tls.build())
        .timeout_global(Some(Duration::from_secs(5)))
        .http_status_as_error(false)
        .user_agent(format!("strata-core/{}", strata_core::VERSION))
        .build()
        .into()
}

fn join_url(base: &str, path: &str) -> String {
    format!("{}{path}", base.trim_end_matches('/'))
}

/// Un appel signé au maître : (statut, corps), réponse vérifiée.
fn call_master(c: &Cluster, m: &MasterRef, path: &str, body: &Value) -> Result<(u16, Value), String> {
    let bytes = body.to_string().into_bytes();
    let date = now_ms();
    let sig = c.sign(&request_msg("POST", path, date, &bytes));
    let res = agent(false, m.ca.as_deref())
        .post(&join_url(&m.url, path))
        .header("content-type", "application/json")
        .header("x-strata-server", &c.id)
        .header("x-strata-date", &date.to_string())
        .header("x-strata-signature", &sig)
        .send(&bytes[..])
        .map_err(|e| e.to_string())?;
    let status = res.status().as_u16();
    let rdate: i64 = res.headers().get("x-strata-date").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0);
    let rsig = res.headers().get("x-strata-signature").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    let rbody = res.into_body().read_to_vec().map_err(|e| e.to_string())?;
    if !verify(&m.key, &response_msg(rdate, &rbody), &rsig) {
        return Err(format!("the answer of {} is not signed by it (HTTP {status})", m.name));
    }
    Ok((status, serde_json::from_slice(&rbody).unwrap_or(Value::Null)))
}

/// Ce que le membre dit de lui.
async fn beat_of(s: &AppState) -> Beat {
    let cfg = s.config.load();
    let host = member_host(s);
    let mut apps = Vec::new();
    for a in s.apps.list().into_iter().filter(|a| a.active && a.compatible) {
        let up = crate::supervisor::answers(a.manifest.port).await;
        apps.push(BeatApp { url: cfg.app_url_at(&a.manifest.id, host.as_deref()), id: a.manifest.id, name: a.manifest.name, icon: a.manifest.icon, up });
    }
    Beat { name: cfg.name.clone(), version: strata_core::VERSION.into(), apps, problems: s.alerts.active().len() }
}

/// Le nom (et le port) sous lequel ce membre s'est rattaché : `strata02.regie.fr:8080`.
fn member_host(s: &AppState) -> Option<String> {
    s.config.load().member_url.as_deref().and_then(|u| u.split("://").nth(1)).map(|r| r.trim_end_matches('/').to_string())
}

/// Le membre bat toutes les 3 s tant que le Core tourne.
pub fn start(s: &Shared) {
    let weak: Weak<AppState> = Arc::downgrade(s);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(BEAT_EVERY).await;
            let Some(s) = weak.upgrade() else { return };
            let Some(m) = s.cluster.master() else { continue };
            let beat = beat_of(&s).await;
            let s2 = s.clone();
            let r = tokio::task::spawn_blocking(move || call_master(&s2.cluster, &m, "/api/cluster/beat", &json!(beat))).await;
            match r {
                Ok(Ok((200, v))) => {
                    *s.cluster.master_ok.lock().unwrap() = Some(Instant::now());
                    *s.cluster.master_error.lock().unwrap() = None;
                    // Les groupes du maître, et son nom.
                    let groups: Vec<(String, String)> = v["groups"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|g| Some((g["id"].as_str()?.to_string(), g["name"].as_str()?.to_string())))
                        .collect();
                    if s.groups.mirror(groups) {
                        tracing::info!("groups of the master copied");
                    }
                    if let Some(name) = v["name"].as_str() {
                        let mut f = s.cluster.file.lock().unwrap();
                        if let Some(mm) = f.master.as_mut().filter(|mm| mm.name != name) {
                            mm.name = name.to_string();
                            let f2 = f.clone();
                            drop(f);
                            let _ = s.cluster.save(&f2);
                        }
                    }
                }
                Ok(Ok((410, _))) => {
                    // Le maître ne nous connaît plus : détaché de là-bas.
                    tracing::warn!("detached by the master");
                    forget_master(&s);
                }
                Ok(Ok((code, v))) => *s.cluster.master_error.lock().unwrap() = Some(v.as_str().map_or_else(|| format!("HTTP {code}"), str::to_string)),
                Ok(Err(e)) => *s.cluster.master_error.lock().unwrap() = Some(e),
                Err(_) => {}
            }
        }
    });
}

fn forget_master(s: &AppState) {
    let mut f = s.cluster.file.lock().unwrap();
    f.master = None;
    let f2 = f.clone();
    drop(f);
    let _ = s.cluster.save(&f2);
    *s.cluster.master_ok.lock().unwrap() = None;
    *s.cluster.master_error.lock().unwrap() = None;
    let mut cfg = (**s.config.load()).clone();
    cfg.member_url = None;
    let _ = cfg.save(&s.data_dir);
    s.config.store(Arc::new(cfg));
}

// ---------- Les routes ----------

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/cluster", get(read))
        .route("/api/cluster/invite", post(invite))
        .route("/api/cluster/enrol", post(enrol))
        .route("/api/cluster/beat", post(beat))
        .route("/api/cluster/leave", post(leave))
        .route("/api/cluster/members/{id}", delete(remove))
        .route("/api/cluster/join", post(join))
        .route("/api/cluster/detach", post(detach))
        .route("/api/cluster/authorize", get(authorize))
        .route("/api/cluster/login", get(login))
        .route("/api/cluster/callback", get(callback))
}

/// Ce serveur et les autres.
async fn read(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let c = &s.cluster;
    let master = c
        .master()
        .map(|m| json!({"id": m.id, "name": m.name, "url": m.url, "joined": m.joined, "up": c.master_up(), "error": c.master_error.lock().unwrap().clone()}));
    let members: Vec<Value> = c
        .live_members()
        .into_iter()
        .map(|(m, b)| json!({"id": m.id, "name": b.as_ref().map_or(m.name.clone(), |b| b.name.clone()), "url": m.url, "joined": m.joined, "up": b.is_some(), "version": b.as_ref().map(|b| b.version.clone()), "apps": b.as_ref().map_or(0, |b| b.apps.len()), "problems": b.as_ref().map_or(0, |b| b.problems)}))
        .collect();
    Ok(Json(json!({"id": c.id, "role": if master.is_some() { "member" } else { "master" }, "master": master, "members": members})))
}

/// Un code d'invitation pour rattacher un serveur.
async fn invite(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    if s.cluster.master().is_some() {
        return Err(bad("this server is a member: invite from its master"));
    }
    let code = new_code();
    let mut inv = s.cluster.invites.lock().unwrap();
    inv.retain(|(_, t)| t.elapsed() < INVITE_TTL);
    inv.push((code.clone(), Instant::now()));
    tracing::info!("invitation made by {}", me.username);
    Ok(Json(json!({"code": code, "expiresAt": now_ms() + INVITE_TTL.as_millis() as i64})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Enrol {
    id: String,
    name: String,
    url: String,
    key: String,
}

/// Côté maître : un serveur se rattache avec un code. Scellé par le code dans les deux sens.
async fn enrol(State(s): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    let given = headers.get("x-strata-mac").and_then(|v| v.to_str().ok()).unwrap_or_default();
    let code = {
        let mut inv = s.cluster.invites.lock().unwrap();
        inv.retain(|(_, t)| t.elapsed() < INVITE_TTL);
        let i = inv.iter().position(|(c, _)| mac_ok(c, &body, given));
        i.map(|i| inv.remove(i).0)
    };
    let Some(code) = code else {
        return (StatusCode::FORBIDDEN, "this code is not valid (wrong, used or expired)").into_response();
    };
    let Ok(e) = serde_json::from_slice::<Enrol>(&body) else { return (StatusCode::BAD_REQUEST, "unreadable").into_response() };
    if e.id == s.cluster.id {
        return (StatusCode::BAD_REQUEST, "a server cannot join itself").into_response();
    }
    // Son identifiant vient de sa clé : un code ne permet pas de prendre la place d'un autre membre.
    if B64.decode(&e.key).map(|k| sha256(&k)[..12].to_string()).ok().as_deref() != Some(e.id.as_str()) {
        return (StatusCode::BAD_REQUEST, "the server id does not match its key").into_response();
    }
    let member = Member { id: e.id.clone(), name: e.name.trim().chars().take(80).collect(), url: e.url.clone(), key: e.key.clone(), joined: now_ms() };
    {
        let mut f = s.cluster.file.lock().unwrap();
        f.members.retain(|m| m.id != member.id);
        f.members.push(member.clone());
        let f2 = f.clone();
        drop(f);
        if let Err(e) = s.cluster.save(&f2) {
            return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
        }
    }
    tracing::info!("{} ({}) joined this cockpit", member.name, member.url);
    let cfg = s.config.load();
    let ca = (cfg.https.mode == crate::config::HttpsMode::Strata).then(|| s.tls.ca_pem()).flatten();
    let answer = json!({"id": s.cluster.id, "name": cfg.name, "key": s.cluster.public_key(), "ca": ca}).to_string();
    let m = mac(&code, answer.as_bytes());
    ([(header::CONTENT_TYPE, "application/json".to_string()), (header::HeaderName::from_static("x-strata-mac"), m)], answer).into_response()
}

/// Côté maître : le battement d'un membre.
async fn beat(State(s): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    let m = match s.cluster.check_member("POST", "/api/cluster/beat", &headers, &body) {
        Ok(m) => m,
        Err((code, e)) => return s.cluster.signed(code, json!(e)),
    };
    let b: Beat = serde_json::from_slice(&body).unwrap_or_default();
    s.cluster.live.lock().unwrap().insert(m.id.clone(), Live { at: Instant::now(), beat: b });
    let groups: Vec<Value> = s.groups.list().into_iter().map(|g| json!({"id": g.id, "name": g.name})).collect();
    s.cluster.signed(StatusCode::OK, json!({"name": s.config.load().name, "groups": groups}))
}

fn drop_member(s: &AppState, id: &str) -> Option<Member> {
    let mut f = s.cluster.file.lock().unwrap();
    let i = f.members.iter().position(|m| m.id == id)?;
    let m = f.members.remove(i);
    let f2 = f.clone();
    drop(f);
    let _ = s.cluster.save(&f2);
    s.cluster.live.lock().unwrap().remove(id);
    Some(m)
}

/// Côté maître : un membre se détache.
async fn leave(State(s): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    match s.cluster.check_member("POST", "/api/cluster/leave", &headers, &body) {
        Ok(m) => {
            drop_member(&s, &m.id);
            tracing::info!("{} left this cockpit", m.name);
            s.cluster.signed(StatusCode::OK, json!({"ok": true}))
        }
        Err((code, e)) => s.cluster.signed(code, json!(e)),
    }
}

/// Côté maître : détacher un membre.
async fn remove(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath(id): UrlPath<String>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let m = drop_member(&s, &id).ok_or_else(|| bad("no such server"))?;
    tracing::info!("{} detached by {}", m.name, me.username);
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Join {
    master_url: String,
    code: String,
}

/// Côté membre : se rattacher au cockpit d'un maître.
async fn join(State(s): State<Shared>, CurrentUser(me): CurrentUser, headers: HeaderMap, Json(b): Json<Join>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    if s.cluster.master().is_some() {
        return Err(bad("this server is already a member: detach it first"));
    }
    if !s.cluster.members().is_empty() {
        return Err(bad("this server has members: detach them first"));
    }
    let master_url = b.master_url.trim().trim_end_matches('/').to_string();
    if !(master_url.starts_with("https://") || master_url.starts_with("http://")) {
        return Err(bad("the address of the master starts with https://"));
    }
    let cfg = s.config.load();
    // L'adresse de ce Core, vue du navigateur qui l'administre (avec son port s'il en a un).
    let my_url = cfg.core_url_at(auth::host(&headers));
    let req = json!({"id": s.cluster.id, "name": cfg.name, "url": my_url, "key": s.cluster.public_key()}).to_string();
    let code = b.code.clone();
    let req_mac = mac(&code, req.as_bytes());
    let url = join_url(&master_url, "/api/cluster/enrol");
    let (status, rmac, body) = blocking(move || -> Result<(u16, String, Vec<u8>), String> {
        let res = agent(true, None)
            .post(&url)
            .header("content-type", "application/json")
            .header("x-strata-mac", &req_mac)
            .send(req.as_bytes())
            .map_err(|e| format!("cannot reach the master: {e}"))?;
        let status = res.status().as_u16();
        let m = res.headers().get("x-strata-mac").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        Ok((status, m, res.into_body().read_to_vec().map_err(|e| e.to_string())?))
    })
    .await?
    .map_err(bad)?;
    if status != 200 {
        let why = String::from_utf8_lossy(&body).into_owned();
        return Err(bad(if why.is_empty() { format!("the master answered HTTP {status}") } else { why }));
    }
    if !mac_ok(&code, &body, &rmac) {
        return Err(bad("the answer does not come from the master (sealed with another code)"));
    }
    let v: Value = serde_json::from_slice(&body).map_err(|_| bad("unreadable answer"))?;
    let m = MasterRef {
        id: v["id"].as_str().unwrap_or_default().into(),
        name: v["name"].as_str().unwrap_or_default().into(),
        url: master_url,
        key: v["key"].as_str().unwrap_or_default().into(),
        ca: v["ca"].as_str().map(str::to_string),
        joined: now_ms(),
    };
    {
        let mut f = s.cluster.file.lock().unwrap();
        f.master = Some(m.clone());
        let f2 = f.clone();
        drop(f);
        s.cluster.save(&f2).map_err(bad)?;
    }
    let mut cfg2 = (**cfg).clone();
    cfg2.member_url = Some(my_url);
    cfg2.save(&s.data_dir).map_err(|e| bad(format!("cannot save the settings: {e}")))?;
    s.config.store(Arc::new(cfg2));
    tracing::info!("joined the cockpit of {} ({}) by {}", m.name, m.url, me.username);
    Ok(Json(json!({"ok": true, "master": {"name": m.name, "url": m.url}})))
}

/// Côté membre : se détacher (le maître est prévenu s'il répond).
async fn detach(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let m = s.cluster.master().ok_or_else(|| bad("this server is not a member"))?;
    let s2 = s.clone();
    let told = blocking(move || call_master(&s2.cluster, &m, "/api/cluster/leave", &json!({})).is_ok()).await?;
    forget_master(&s);
    tracing::info!("detached from its master by {}", me.username);
    Ok(Json(json!({"ok": true, "told": told})))
}

// ---------- La connexion unique ----------

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct Token {
    iss: String,
    aud: String,
    exp: i64,
    nonce: String,
    user: TokenUser,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct TokenUser {
    id: String,
    username: String,
    display_name: String,
    admin: bool,
    groups: Vec<String>,
    lang: String,
    theme: String,
}

#[derive(Deserialize)]
struct AuthorizeQuery {
    server: String,
    state: String,
    #[serde(default)]
    next: String,
}

/// Côté maître : un membre demande qui est l'utilisateur. Connecté : retour aussitôt au membre avec
/// un jeton signé ; sinon, la connexion du cockpit d'abord, qui ramène ici.
async fn authorize(State(s): State<Shared>, MaybeUser(user): MaybeUser, headers: HeaderMap, Query(q): Query<AuthorizeQuery>) -> Response {
    let Some(m) = s.cluster.members().into_iter().find(|m| m.id == q.server) else {
        return (StatusCode::BAD_REQUEST, "unknown server").into_response();
    };
    let Some(u) = user else {
        let here = format!("/api/cluster/authorize?server={}&state={}&next={}", enc(&q.server), enc(&q.state), enc(&q.next));
        let cfg = s.config.load();
        return Redirect::to(&format!("{}?next={}", cfg.core_url_at(auth::host(&headers)), enc(&here))).into_response();
    };
    let t = Token {
        iss: s.cluster.id.clone(),
        aud: m.id.clone(),
        exp: now_ms() + TOKEN_MS,
        nonce: q.state.chars().take(64).collect(),
        user: TokenUser {
            id: u.id.clone(),
            username: u.username.clone(),
            display_name: u.display_name.clone(),
            admin: u.admin,
            groups: u.groups.clone(),
            lang: u.lang.clone(),
            theme: u.theme.clone(),
        },
    };
    let payload = B64.encode(serde_json::to_vec(&t).unwrap_or_default());
    let sig = s.cluster.sign(payload.as_bytes());
    Redirect::to(&format!("{}?token={payload}.{sig}&next={}", join_url(&m.url, "/api/cluster/callback"), enc(&q.next))).into_response()
}

#[derive(Deserialize)]
struct LoginQuery {
    #[serde(default)]
    next: String,
}

fn state_cookie(v: &str, headers: &HeaderMap) -> HeaderValue {
    let secure = if auth::is_https(headers) { "; Secure" } else { "" };
    let age = if v.is_empty() { 0 } else { 300 };
    HeaderValue::from_str(&format!("{STATE_COOKIE}={v}; Path=/api/cluster; HttpOnly; SameSite=Lax; Max-Age={age}{secure}")).expect("cookie")
}

/// Côté membre : se connecter par le maître (ou, s'il ne répond pas, ici avec un compte local).
async fn login(State(s): State<Shared>, headers: HeaderMap, Query(q): Query<LoginQuery>) -> Response {
    let cfg = s.config.load();
    let local = format!("{}?next={}", cfg.core_url_at(auth::host(&headers)), enc(&q.next));
    let Some(m) = s.cluster.master().filter(|_| s.cluster.master_up()) else {
        return Redirect::to(&format!("{local}&local=1")).into_response();
    };
    let mut b = [0u8; 16];
    let _ = SystemRandom::new().fill(&mut b);
    let state = hex(&b);
    let url = format!("{}?server={}&state={state}&next={}", join_url(&m.url, "/api/cluster/authorize"), enc(&s.cluster.id), enc(&q.next));
    let mut res = Redirect::to(&url).into_response();
    res.headers_mut().insert(header::SET_COOKIE, state_cookie(&state, &headers));
    res
}

#[derive(Deserialize)]
struct CallbackQuery {
    token: String,
    #[serde(default)]
    next: String,
}

/// Côté membre : le maître revient avec le jeton ; la session s'ouvre ici.
async fn callback(State(s): State<Shared>, headers: HeaderMap, Query(q): Query<CallbackQuery>) -> Response {
    let fail = |why: &str| {
        tracing::warn!("sign-in through the master refused: {why}");
        (StatusCode::FORBIDDEN, format!("sign-in through the master refused: {why}")).into_response()
    };
    let Some(m) = s.cluster.master() else { return fail("not a member") };
    let Some((payload, sig)) = q.token.split_once('.') else { return fail("unreadable token") };
    if !verify(&m.key, payload.as_bytes(), sig) {
        return fail("bad signature");
    }
    let Some(t) = B64.decode(payload).ok().and_then(|b| serde_json::from_slice::<Token>(&b).ok()) else { return fail("unreadable token") };
    let state = auth::cookie(&headers, STATE_COOKIE).unwrap_or_default();
    if t.iss != m.id || t.aud != s.cluster.id || t.exp < now_ms() || state.is_empty() || t.nonce != state {
        return fail("token not for this server, expired or not asked here");
    }
    {
        let mut used = s.cluster.used.lock().unwrap();
        used.retain(|_, exp| *exp > now_ms());
        if used.insert(t.nonce.clone(), t.exp).is_some() {
            return fail("token already used");
        }
    }
    let u = t.user;
    let key = format!("master|{}", u.id);
    let groups: Vec<String> = u.groups.into_iter().filter(|g| s.groups.exists(g)).collect();
    let r = s.auth.edit(|list| match list.iter().position(|x| x.sso == key) {
        Some(i) => {
            let x = &mut list[i];
            if x.disabled {
                return Err("disabled".into());
            }
            x.display_name = if u.display_name.is_empty() { u.username.clone() } else { u.display_name.clone() };
            x.admin = u.admin;
            x.groups = groups.clone();
            x.lang = u.lang.clone();
            x.theme = u.theme.clone();
            Ok(x.clone())
        }
        None => {
            let mut x = User::from_directory(crate::sso::free_username(&u.username, list), u.display_name.clone(), u.admin, groups.clone(), key.clone());
            x.lang = u.lang.clone();
            x.theme = u.theme.clone();
            list.push(x.clone());
            Ok(x)
        }
    });
    let user = match r {
        Ok(u) => u,
        Err(e) if e == "disabled" => return fail("this account is disabled on this server"),
        Err(e) => return fail(&e),
    };
    tracing::info!("{} signed in through {}", user.username, m.name);
    let token = s.auth.open_session(&user.id);
    let cfg = s.config.load();
    let domain = cfg.cookie_domain(auth::host(&headers));
    let mut res = Redirect::to(&cfg.safe_return(Some(&q.next))).into_response();
    res.headers_mut().insert(header::SET_COOKIE, auth::session_cookie(&token, &headers, domain.as_deref()));
    res.headers_mut().append(header::SET_COOKIE, state_cookie("", &headers));
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_macs_and_signatures() {
        let code = new_code();
        assert_eq!(code.len(), 12);
        assert_eq!(clean_code(&code.to_lowercase().replace('-', " ")), clean_code(&code));
        let m = mac(&code, b"hello");
        assert!(mac_ok(&code.to_lowercase(), b"hello", &m));
        assert!(!mac_ok(&code, b"hellO", &m));
        assert!(!mac_ok("AAAA-AAAA-AA", b"hello", &m));
        let dir = std::env::temp_dir().join(format!("strata-cluster-{}", strata_core::new_id()));
        let c = Cluster::load(&dir);
        let again = Cluster::load(&dir);
        assert_eq!(c.id, again.id, "the key is kept");
        let sig = c.sign(b"msg");
        assert!(verify(&c.public_key(), b"msg", &sig));
        assert!(!verify(&c.public_key(), b"msG", &sig));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
