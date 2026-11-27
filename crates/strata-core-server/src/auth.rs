//! Comptes (`users.json`), mots de passe (Argon2), sessions, connexion. Repris de LiveCut.
//!
//! Le Core gère **qui sont les utilisateurs** : comptes, groupes, et un seul droit à lui,
//! administrateur des serveurs. Ce qu'un utilisateur peut faire dans une appli se règle dans l'appli.
//! `users.json` est relu quand il change sur disque (la commande `strata-core user …` agit pendant
//! que le serveur tourne).
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::{ConnectInfo, FromRequestParts, State},
    http::{header, request::Parts, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    convert::Infallible,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant, SystemTime},
};
use tokio::sync::Semaphore;

use crate::{
    api::{bad, blocking, ApiResult},
    guard,
    state::Shared,
};

pub const COOKIE: &str = "strata_session";
/// Durée glissante d'une session.
pub const SESSION_TTL: Duration = Duration::from_secs(14 * 24 * 3600);
pub const MIN_PASSWORD: usize = 8;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub display_name: String,
    /// Administrateur des serveurs : les réglages du cockpit.
    #[serde(default)]
    pub admin: bool,
    /// Les groupes (créés dans le Core ou venus de l'annuaire) : les applis y donnent des droits.
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub disabled: bool,
    /// "en" | "fr" ; vide : celle du navigateur.
    #[serde(default)]
    pub lang: String,
    /// "dark" | "light" | "system" ; vide : sombre.
    #[serde(default)]
    pub theme: String,
    /// « émetteur|sujet » d'un compte de l'annuaire (SSO).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sso: String,
    /// Compte de l'annuaire : son rôle d'administrateur est fixé à la main, l'annuaire n'y touche plus.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub admin_fixed: bool,
    /// Jamais envoyé au navigateur (voir `PublicUser`) ; vide : pas de connexion par mot de passe.
    #[serde(default)]
    password_hash: String,
}

impl User {
    pub fn name(&self) -> &str {
        if self.display_name.is_empty() {
            &self.username
        } else {
            &self.display_name
        }
    }
    /// Ce que l'API envoie de ce compte (les noms de ses groupes compris).
    pub fn public(&self, groups: &crate::groups::Groups) -> PublicUser {
        PublicUser::new(self, groups)
    }
}

/// Un utilisateur tel que l'API l'envoie.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PublicUser {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub admin: bool,
    /// Les identifiants de ses groupes.
    pub groups: Vec<String>,
    /// Leurs noms.
    pub group_names: Vec<String>,
    pub disabled: bool,
    pub lang: String,
    pub theme: String,
    pub sso: bool,
    /// Son rôle d'administrateur est fixé à la main (l'annuaire n'y touche plus).
    pub admin_fixed: bool,
}

impl PublicUser {
    pub fn new(u: &User, groups: &crate::groups::Groups) -> Self {
        PublicUser {
            id: u.id.clone(),
            username: u.username.clone(),
            display_name: u.name().to_string(),
            admin: u.admin,
            groups: u.groups.clone(),
            group_names: groups.names_of(&u.groups),
            disabled: u.disabled,
            lang: u.lang.clone(),
            theme: u.theme.clone(),
            sso: !u.sso.is_empty(),
            admin_fixed: u.admin_fixed,
        }
    }
}

impl User {
    /// Un compte de l'annuaire (SSO) : pas de mot de passe.
    pub fn from_directory(username: String, display_name: String, admin: bool, groups: Vec<String>, sso: String) -> User {
        User {
            id: strata_core::new_id(),
            display_name: if display_name.trim().is_empty() { username.clone() } else { display_name.trim().to_string() },
            username,
            admin,
            groups,
            disabled: false,
            lang: String::new(),
            theme: String::new(),
            sso,
            admin_fixed: false,
            password_hash: String::new(),
        }
    }
}

/// Un nouveau compte.
pub struct NewUser {
    pub username: String,
    pub display_name: String,
    pub admin: bool,
    /// None : pas de mot de passe (connexion par l'annuaire seulement).
    pub password: Option<String>,
}

/// Lettres, chiffres et `. - _ @`, 64 au plus.
pub fn validate_username(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("the user name cannot be empty".into());
    }
    if name.chars().count() > 64 {
        return Err("the user name is too long (64 characters at most)".into());
    }
    if !name.chars().all(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '@')) {
        return Err("the user name can only have letters, digits and . - _ @".into());
    }
    Ok(())
}

pub fn validate_password(pw: &str) -> Result<(), String> {
    if pw.chars().count() < MIN_PASSWORD {
        return Err(format!("the password needs at least {MIN_PASSWORD} characters"));
    }
    Ok(())
}

/// Argon2 (paramètres par défaut), sel de 16 octets d'un UUID v4.
pub fn hash_password(pw: &str) -> String {
    let salt = SaltString::encode_b64(uuid::Uuid::new_v4().as_bytes()).expect("16-byte salt");
    Argon2::default().hash_password(pw.as_bytes(), &salt).expect("argon2").to_string()
}

pub fn verify_hash(hash: &str, pw: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|h| Argon2::default().verify_password(pw.as_bytes(), &h).is_ok())
}

/// Un hachage vérifié pour les noms inconnus : le temps de réponse ne dit rien.
fn dummy_hash() -> &'static str {
    static H: OnceLock<String> = OnceLock::new();
    H.get_or_init(|| hash_password("not a real password"))
}

pub fn new_token() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

struct Session {
    user_id: String,
    expires: Instant,
}

struct Store {
    list: Vec<User>,
    /// (date, taille) de users.json à la dernière lecture : relu quand il change.
    stamp: Option<(SystemTime, u64)>,
}

pub struct Auth {
    path: PathBuf,
    store: Mutex<Store>,
    sessions: Mutex<HashMap<String, Session>>,
    verify_slots: Semaphore,
    pub logins: guard::Logins,
}

fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// Reste-t-il un administrateur actif ?
pub fn has_active_admin(list: &[User]) -> bool {
    list.iter().any(|u| u.admin && !u.disabled)
}

fn check_list(list: &[User]) -> Result<(), String> {
    let mut names = HashSet::new();
    for u in list {
        if !names.insert(u.username.to_lowercase()) {
            return Err("this user name is already taken".into());
        }
    }
    if !list.is_empty() && !has_active_admin(list) {
        return Err("there must stay at least one active admin".into());
    }
    Ok(())
}

impl Auth {
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("users.json");
        let list = strata_core::files::read_json(&path).unwrap_or_default();
        Auth {
            store: Mutex::new(Store { list, stamp: stamp(&path) }),
            path,
            sessions: Mutex::new(HashMap::new()),
            verify_slots: Semaphore::new(4),
            logins: guard::Logins::default(),
        }
    }

    /// Relit users.json s'il a changé ; renvoie les comptes dont les sessions s'arrêtent.
    fn refresh(&self, store: &mut Store) -> Vec<String> {
        let now = stamp(&self.path);
        if now == store.stamp {
            return Vec::new();
        }
        store.stamp = now;
        let list: Vec<User> = if now.is_none() {
            Vec::new()
        } else {
            match strata_core::files::read_json(&self.path) {
                Some(l) => l,
                None => return Vec::new(),
            }
        };
        let ended = ended_sessions(&store.list, &list);
        store.list = list;
        ended
    }

    fn with_store<R>(&self, f: impl FnOnce(&Store) -> R) -> R {
        let mut store = self.store.lock().unwrap();
        let ended = self.refresh(&mut store);
        let r = f(&store);
        drop(store);
        self.end_sessions(&ended);
        r
    }

    pub fn needs_setup(&self) -> bool {
        self.with_store(|s| s.list.is_empty())
    }

    pub fn users(&self) -> Vec<User> {
        self.with_store(|s| s.list.clone())
    }

    pub fn get(&self, id: &str) -> Option<User> {
        self.with_store(|s| s.list.iter().find(|u| u.id == id).cloned())
    }

    pub fn find_name(&self, name: &str) -> Option<User> {
        self.with_store(|s| {
            let name = name.to_lowercase();
            s.list.iter().find(|u| u.username.to_lowercase() == name).cloned()
        })
    }

    /// Change la liste d'un coup : règles vérifiées, enregistrée (0600), sessions arrêtées si besoin.
    pub fn edit<R>(&self, f: impl FnOnce(&mut Vec<User>) -> Result<R, String>) -> Result<R, String> {
        let mut store = self.store.lock().unwrap();
        let mut ended = self.refresh(&mut store);
        let mut list = store.list.clone();
        let r = f(&mut list)?;
        check_list(&list)?;
        strata_core::files::write_json(&self.path, &list, true).map_err(|e| format!("cannot save the users: {e}"))?;
        store.stamp = stamp(&self.path);
        ended.extend(ended_sessions(&store.list, &list));
        store.list = list;
        drop(store);
        self.end_sessions(&ended);
        Ok(r)
    }

    /// Ajoute un compte (hache le mot de passe : à appeler hors des fils asynchrones).
    pub fn add(&self, n: NewUser) -> Result<User, String> {
        let username = n.username.trim().to_string();
        validate_username(&username)?;
        let hash = match &n.password {
            Some(p) => {
                validate_password(p)?;
                hash_password(p)
            }
            None => String::new(),
        };
        let display_name = n.display_name.trim();
        let user = User {
            id: strata_core::new_id(),
            display_name: if display_name.is_empty() { username.clone() } else { display_name.to_string() },
            username,
            admin: n.admin,
            groups: Vec::new(),
            disabled: false,
            lang: String::new(),
            theme: String::new(),
            sso: String::new(),
            admin_fixed: false,
            password_hash: hash,
        };
        let u = user.clone();
        self.edit(move |list| {
            list.push(u);
            Ok(())
        })?;
        Ok(user)
    }

    /// Change un compte.
    pub fn modify(&self, id: &str, f: impl FnOnce(&mut User) -> Result<(), String>) -> Result<User, String> {
        self.edit(|list| {
            let u = list.iter_mut().find(|u| u.id == id).ok_or_else(|| "no such user".to_string())?;
            f(u)?;
            Ok(u.clone())
        })
    }

    pub fn delete(&self, id: &str) -> Result<User, String> {
        self.edit(|list| {
            let i = list.iter().position(|u| u.id == id).ok_or_else(|| "no such user".to_string())?;
            let u = list.remove(i);
            if !has_active_admin(list) {
                return Err("you cannot delete the last active admin".into());
            }
            Ok(u)
        })
    }

    /// Change un mot de passe (hache : hors des fils asynchrones). Arrête les sessions du compte.
    pub fn set_password(&self, id: &str, pw: &str) -> Result<User, String> {
        validate_password(pw)?;
        let hash = hash_password(pw);
        self.modify(id, |u| {
            u.password_hash = hash;
            Ok(())
        })
    }

    /// Le compte qui a ce nom et ce mot de passe (bloquant : Argon2).
    pub fn check_password(&self, username: &str, password: &str) -> Option<User> {
        let user = self.find_name(username.trim());
        match &user {
            Some(u) if !u.password_hash.is_empty() => verify_hash(&u.password_hash, password).then_some(user.clone()).flatten(),
            _ => {
                let _ = verify_hash(dummy_hash(), password);
                None
            }
        }
    }

    /// `check_password` hors des fils asynchrones, 4 à la fois au plus.
    pub async fn verify(self: &Arc<Self>, username: &str, password: &str) -> Option<User> {
        let _slot = self.verify_slots.acquire().await.ok()?;
        let (me, u, p) = (self.clone(), username.to_string(), password.to_string());
        tokio::task::spawn_blocking(move || me.check_password(&u, &p)).await.ok().flatten()
    }

    /// Ouvre une session.
    pub fn open_session(&self, user_id: &str) -> String {
        let token = new_token();
        let now = Instant::now();
        let mut sessions = self.sessions.lock().unwrap();
        sessions.retain(|_, s| s.expires > now);
        sessions.insert(token.clone(), Session { user_id: user_id.to_string(), expires: now + SESSION_TTL });
        token
    }

    pub fn close_session(&self, token: &str) {
        self.sessions.lock().unwrap().remove(token);
    }

    fn end_sessions(&self, user_ids: &[String]) {
        if user_ids.is_empty() {
            return;
        }
        self.sessions.lock().unwrap().retain(|_, s| !user_ids.contains(&s.user_id));
    }

    /// Le compte d'une session (et la session prolongée).
    pub fn user_for_token(&self, token: &str) -> Option<User> {
        let now = Instant::now();
        let user_id = {
            let mut sessions = self.sessions.lock().unwrap();
            let s = sessions.get_mut(token)?;
            if s.expires <= now {
                sessions.remove(token);
                return None;
            }
            s.expires = now + SESSION_TTL;
            s.user_id.clone()
        };
        self.get(&user_id)
    }
}

/// Les comptes dont les sessions s'arrêtent : supprimés, désactivés, nouveau mot de passe.
fn ended_sessions(old: &[User], new: &[User]) -> Vec<String> {
    old.iter()
        .filter(|o| match new.iter().find(|n| n.id == o.id) {
            None => true,
            Some(n) => (n.disabled && !o.disabled) || n.password_hash != o.password_hash,
        })
        .map(|o| o.id.clone())
        .collect()
}

/// `Authorization: Bearer <jeton>`, sinon le cookie de session.
pub fn token_from(headers: &HeaderMap) -> Option<String> {
    if let Some(v) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if let Some(t) = v.strip_prefix("Bearer ") {
            return Some(t.trim().to_string());
        }
    }
    cookie(headers, COOKIE)
}

/// La valeur d'un cookie (aucune si vide).
pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all("cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|c| c.trim().strip_prefix(name).and_then(|v| v.strip_prefix('=')).map(str::to_string))
        .filter(|t| !t.is_empty())
}

/// La requête est-elle arrivée en HTTPS (directement ou par un relais) ?
pub fn is_https(headers: &HeaderMap) -> bool {
    headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()).is_some_and(|v| v.split(',').next().unwrap_or("").trim().eq_ignore_ascii_case("https"))
}

/// Le cookie de session ; `domain` : le nom du serveur, pour que la session vaille aussi dans ses
/// sous-domaines (les applis).
pub fn session_cookie(token: &str, headers: &HeaderMap, domain: Option<&str>) -> HeaderValue {
    let max_age = if token.is_empty() { 0 } else { SESSION_TTL.as_secs() };
    let secure = if is_https(headers) { "; Secure" } else { "" };
    let domain = domain.map(|d| format!("; Domain={d}")).unwrap_or_default();
    HeaderValue::from_str(&format!("{COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{domain}{secure}")).expect("cookie")
}

pub fn host(headers: &HeaderMap) -> Option<&str> {
    headers.get("x-forwarded-host").or_else(|| headers.get("host")).and_then(|v| v.to_str().ok())
}

/// L'utilisateur connecté, sinon 401 « login required ».
pub struct CurrentUser(pub User);

impl FromRequestParts<Shared> for CurrentUser {
    type Rejection = (StatusCode, String);
    async fn from_request_parts(parts: &mut Parts, s: &Shared) -> Result<Self, Self::Rejection> {
        token_from(&parts.headers)
            .and_then(|t| s.auth.user_for_token(&t))
            .filter(|u| !u.disabled)
            .map(CurrentUser)
            .ok_or((StatusCode::UNAUTHORIZED, "login required".into()))
    }
}

/// L'utilisateur connecté, s'il y en a un.
pub struct MaybeUser(pub Option<User>);

impl FromRequestParts<Shared> for MaybeUser {
    type Rejection = Infallible;
    async fn from_request_parts(parts: &mut Parts, s: &Shared) -> Result<Self, Self::Rejection> {
        Ok(MaybeUser(token_from(&parts.headers).and_then(|t| s.auth.user_for_token(&t)).filter(|u| !u.disabled)))
    }
}

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/auth/state", get(state))
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/setup", post(setup))
}

/// Ouvre une session : `{user, token}` et le cookie.
pub fn signed_in(s: &Shared, user: &User, headers: &HeaderMap) -> Response {
    let token = s.auth.open_session(&user.id);
    let domain = s.config.load().cookie_domain(host(headers));
    let mut res = Json(json!({"user": user.public(&s.groups), "token": token})).into_response();
    res.headers_mut().insert(header::SET_COOKIE, session_cookie(&token, headers, domain.as_deref()));
    res
}

async fn state(State(s): State<Shared>, MaybeUser(user): MaybeUser) -> Json<serde_json::Value> {
    let cfg = s.config.load();
    Json(json!({
        "needsSetup": s.auth.needs_setup(),
        "user": user.as_ref().map(|u| u.public(&s.groups)),
        "serverName": cfg.name,
        "timezone": cfg.timezone,
        "version": strata_core::VERSION,
        "sso": cfg.sso.active().then(|| cfg.sso.button_name()),
        "localAccounts": cfg.sso.local_accounts_now(),
    }))
}

#[derive(Deserialize)]
struct LoginBody {
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
}

async fn login(State(s): State<Shared>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(b): Json<LoginBody>) -> ApiResult<Response> {
    let ip = guard::client_ip(peer, &headers);
    let name = b.username.trim().to_string();
    if let Err(wait) = s.auth.logins.begin(ip, &name) {
        return Err((StatusCode::TOO_MANY_REQUESTS, format!("too many tries: wait {wait} s")));
    }
    // Un compte qui doit passer par l'annuaire : dit avant de regarder le mot de passe (la réponse ne
    // dit jamais si un mot de passe était juste).
    let sso = s.config.load().sso.clone();
    if let Some(u) = s.auth.find_name(&name) {
        if !sso.local_allowed(u.admin) && !guard::this_machine(peer, &headers) {
            return Err((StatusCode::FORBIDDEN, format!("sign in with {}", sso.button_name())));
        }
    }
    let user = s.auth.verify(&name, &b.password).await.filter(|u| !u.disabled);
    let Some(user) = user else {
        return Err((StatusCode::UNAUTHORIZED, "wrong user name or password".into()));
    };
    s.auth.logins.succeeded(ip, &name);
    tracing::info!("{} signed in from {ip}", user.username);
    Ok(signed_in(&s, &user, &headers))
}

async fn logout(State(s): State<Shared>, headers: HeaderMap) -> Response {
    if let Some(t) = token_from(&headers) {
        s.auth.close_session(&t);
    }
    let domain = s.config.load().cookie_domain(host(&headers));
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut().insert(header::SET_COOKIE, session_cookie("", &headers, domain.as_deref()));
    res
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetupBody {
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    display_name: String,
}

/// Le premier compte (administrateur), depuis le serveur lui-même seulement.
async fn setup(State(s): State<Shared>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(b): Json<SetupBody>) -> ApiResult<Response> {
    if !s.auth.needs_setup() {
        return Err((StatusCode::CONFLICT, "already set up".into()));
    }
    if !guard::this_machine(peer, &headers) {
        return Err((StatusCode::FORBIDDEN, "the first account is created on the server itself (http://localhost)".into()));
    }
    let auth = s.auth.clone();
    let user = blocking(move || {
        if !auth.needs_setup() {
            return Err((StatusCode::CONFLICT, "already set up".to_string()));
        }
        auth.add(NewUser { username: b.username, display_name: b.display_name, admin: true, password: Some(b.password) }).map_err(bad)
    })
    .await??;
    tracing::info!("admin account {} created", user.username);
    Ok(signed_in(&s, &user, &headers))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify() {
        let h = hash_password("correct horse");
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_hash(&h, "correct horse"));
        assert!(!verify_hash(&h, "wrong horse"));
        assert!(!verify_hash("", "anything"));
        assert_ne!(h, hash_password("correct horse"), "salted");
    }

    #[test]
    fn usernames() {
        assert!(validate_username("admin").is_ok());
        assert!(validate_username("jean.dupont@example.com").is_ok());
        assert!(validate_username("éloïse").is_ok());
        assert!(validate_username("").is_err());
        assert!(validate_username("a b").is_err());
        assert!(validate_username(&"x".repeat(65)).is_err());
        assert!(validate_password("1234567").is_err());
        assert!(validate_password("12345678").is_ok());
    }

    #[test]
    fn accounts_and_sessions() {
        let dir = std::env::temp_dir().join(format!("strata-auth-{}", strata_core::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let auth = Auth::load(&dir);
        assert!(auth.needs_setup());
        let nu = |name: &str, admin| NewUser { username: name.into(), display_name: String::new(), admin, password: Some("password1".into()) };
        let admin = auth.add(nu("admin", true)).unwrap();
        assert!(!auth.needs_setup());
        assert!(auth.add(nu("ADMIN", false)).is_err(), "unique without case");
        let v = auth.add(nu("claire", false)).unwrap();
        assert!(auth.check_password("Admin", "password1").is_some());
        assert!(auth.check_password("admin", "nope").is_none());
        assert!(auth.check_password("ghost", "password1").is_none());
        // le dernier administrateur actif reste
        assert!(auth
            .modify(&admin.id, |u| {
                u.disabled = true;
                Ok(())
            })
            .is_err());
        assert!(auth.delete(&admin.id).is_err());
        // un nouveau mot de passe arrête les sessions
        let t = auth.open_session(&v.id);
        assert_eq!(auth.user_for_token(&t).unwrap().id, v.id);
        auth.set_password(&v.id, "password2").unwrap();
        assert!(auth.user_for_token(&t).is_none());
        // relu depuis le disque
        let again = Auth::load(&dir);
        assert_eq!(again.users().len(), 2);
        assert!(again.check_password("claire", "password2").is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cookie_with_domain() {
        let h = HeaderMap::new();
        let c = session_cookie("abc", &h, Some("strata01.local"));
        let c = c.to_str().unwrap();
        assert!(c.starts_with("strata_session=abc;"));
        assert!(c.contains("; Domain=strata01.local"));
        assert!(c.contains("HttpOnly"));
        assert!(!c.contains("Secure"));
        assert!(session_cookie("", &h, None).to_str().unwrap().contains("Max-Age=0"));
    }
}
