//! La connexion par l'annuaire de l'entreprise : OpenID Connect (Microsoft Entra, Okta, Keycloak,
//! Google…). Repris de LiveCut, adapté au Core : les groupes de l'annuaire donnent les groupes du Core
//! et le rôle d'administrateur des serveurs.
//!
//! Le parcours : `/api/auth/sso/start` envoie le navigateur chez le fournisseur (code d'autorisation
//! avec PKCE) ; il revient sur `/api/auth/sso/callback`, où le jeton d'identité est vérifié (signature
//! par les clés du fournisseur, émetteur, destinataire, expiration, nonce), puis le compte trouvé ou
//! créé, avec ses groupes et son rôle.
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::rand::SecureRandom;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    api::{bad, blocking, require, ApiResult},
    auth::{self, CurrentUser, User},
    groups::clean_directory_group,
    state::Shared,
};

/// Un secret tel que l'API le renvoie.
pub const MASK: &str = "••••••••";
pub const PROVIDERS: [&str; 5] = ["entra", "okta", "keycloak", "google", "other"];

/// Qui peut encore se connecter avec un mot de passe local quand le SSO est actif.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LocalAccounts {
    /// Personne (sauf depuis le serveur lui-même, pour réparer).
    #[serde(rename = "none")]
    Off,
    /// Les administrateurs seulement (une porte de secours).
    #[default]
    #[serde(rename = "adminOnly")]
    AdminOnly,
    #[serde(rename = "all")]
    All,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct SsoConfig {
    pub enabled: bool,
    pub provider: String,
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    /// La revendication du jeton qui porte les groupes.
    pub groups_claim: String,
    /// Les groupes de l'annuaire qui font un administrateur des serveurs.
    pub admin_groups: Vec<String>,
    /// Accepter aussi les comptes de l'annuaire qui ne sont dans aucun groupe connu.
    pub allow_any: bool,
    pub local_accounts: LocalAccounts,
    /// Les domaines d'e-mail autorisés (vide : tous ; Google en exige, sinon tout compte Google entre).
    pub domains: Vec<String>,
}

impl Default for SsoConfig {
    fn default() -> Self {
        SsoConfig {
            enabled: false,
            provider: "entra".into(),
            issuer: String::new(),
            client_id: String::new(),
            client_secret: String::new(),
            groups_claim: "groups".into(),
            admin_groups: Vec::new(),
            allow_any: false,
            local_accounts: LocalAccounts::default(),
            domains: Vec::new(),
        }
    }
}

impl SsoConfig {
    pub fn normalize(&mut self) {
        self.provider = self.provider.trim().to_lowercase();
        if !PROVIDERS.contains(&self.provider.as_str()) {
            self.provider = "other".into();
        }
        self.issuer = self.issuer.trim().trim_end_matches('/').to_string();
        self.client_id = self.client_id.trim().to_string();
        self.client_secret = self.client_secret.trim().to_string();
        self.groups_claim = self.groups_claim.trim().to_string();
        if self.groups_claim.is_empty() {
            self.groups_claim = "groups".into();
        }
        self.admin_groups = self.admin_groups.iter().map(|g| g.trim().to_string()).filter(|g| !g.is_empty()).collect();
        self.domains = self.domains.iter().map(|d| d.trim().trim_start_matches('@').to_lowercase()).filter(|d| !d.is_empty()).collect();
    }

    /// Actif, avec un émetteur et un identifiant client.
    pub fn active(&self) -> bool {
        self.enabled && !self.issuer.is_empty() && !self.client_id.is_empty()
    }

    /// Le nom sur le bouton de connexion.
    pub fn button_name(&self) -> &'static str {
        match self.provider.as_str() {
            "entra" => "Microsoft",
            "okta" => "Okta",
            "keycloak" => "Keycloak",
            "google" => "Google",
            _ => "SSO",
        }
    }

    /// Un compte (administrateur ou non) peut-il se connecter avec un mot de passe ?
    pub fn local_allowed(&self, admin: bool) -> bool {
        if !self.active() {
            return true;
        }
        match self.local_accounts {
            LocalAccounts::Off => false,
            LocalAccounts::AdminOnly => admin,
            LocalAccounts::All => true,
        }
    }

    /// Ce que la page de connexion doit savoir : qui peut utiliser un compte local en ce moment.
    pub fn local_accounts_now(&self) -> LocalAccounts {
        if self.active() {
            self.local_accounts
        } else {
            LocalAccounts::All
        }
    }

    /// Ces groupes de l'annuaire font-ils un administrateur des serveurs ?
    pub fn is_admin(&self, directory_groups: &[String]) -> bool {
        let mine: Vec<String> = directory_groups.iter().map(|g| clean_directory_group(g)).collect();
        self.admin_groups.iter().any(|a| mine.contains(&clean_directory_group(a)))
    }
}

impl SsoConfig {
    /// L'adresse garantie de cette identité est-elle dans un domaine autorisé ? (Aucune liste : oui,
    /// sauf pour Google.)
    pub fn domain_allowed(&self, id: &Identity) -> bool {
        if self.domains.is_empty() {
            return self.provider != "google";
        }
        id.address.as_deref().and_then(|a| a.rsplit_once('@')).is_some_and(|(_, d)| self.domains.contains(&d.to_lowercase()))
    }
}

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/auth/sso/start", get(start))
        .route("/api/auth/sso/callback", get(callback))
        .route("/api/sso", get(read).put(write))
        .route("/api/sso/test", post(test))
}

// ---------------------------------------------------------------- découverte

#[derive(Clone, Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
}

/// Un client HTTP avec les certificats du système (proxys d'entreprise), les erreurs HTTP en réponses.
fn agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_global(Some(Duration::from_secs(30)))
        .max_redirects(5)
        .http_status_as_error(false)
        .user_agent(format!("strata-core/{}", strata_core::VERSION))
        .build()
        .into()
}

fn get_json(url: &str) -> Result<Value, String> {
    let mut res = agent().get(url).header("Accept", "application/json").call().map_err(|e| format!("{url}: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("{url}: HTTP {}", res.status().as_u16()));
    }
    let text = res.body_mut().read_to_string().map_err(|e| format!("{url}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{url}: {e}"))
}

fn discover(issuer: &str, fresh: bool) -> Result<Discovery, String> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, Discovery)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if !fresh {
        if let Some((at, d)) = cache.lock().unwrap().get(issuer) {
            if at.elapsed() < Duration::from_secs(3600) {
                return Ok(d.clone());
            }
        }
    }
    let url = format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/'));
    let d: Discovery = serde_json::from_value(get_json(&url)?).map_err(|e| format!("{url}: {e}"))?;
    // L'émetteur annoncé est celui réglé (OpenID Connect Discovery § 4.3) ; les jetons sont
    // vérifiés contre lui.
    if d.issuer.trim_end_matches('/') != issuer.trim_end_matches('/') {
        return Err(format!("{url} announces another issuer ({})", d.issuer));
    }
    cache.lock().unwrap().insert(issuer.to_string(), (Instant::now(), d.clone()));
    Ok(d)
}

fn jwks(uri: &str, fresh: bool) -> Result<Vec<Value>, String> {
    static CACHE: OnceLock<Mutex<HashMap<String, Vec<Value>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if !fresh {
        if let Some(k) = cache.lock().unwrap().get(uri) {
            return Ok(k.clone());
        }
    }
    let keys = get_json(uri)?.get("keys").and_then(Value::as_array).cloned().ok_or_else(|| format!("{uri}: no keys"))?;
    cache.lock().unwrap().insert(uri.to_string(), keys.clone());
    Ok(keys)
}

// ---------------------------------------------------------------- départ

struct Pending {
    nonce: String,
    verifier: String,
    redirect_uri: String,
    next: String,
    created: Instant,
}

const PENDING_TTL: Duration = Duration::from_secs(600);
/// Connexions commencées et pas finies, gardées au plus (les plus anciennes partent).
const PENDING_MAX: usize = 1000;
/// Le navigateur qui a commencé une connexion : lui seul peut la finir.
const STATE_COOKIE: &str = "strata_sso";

fn state_cookie(state: &str, headers: &HeaderMap) -> axum::http::HeaderValue {
    let max_age = if state.is_empty() { 0 } else { PENDING_TTL.as_secs() };
    let secure = if auth::is_https(headers) { "; Secure" } else { "" };
    axum::http::HeaderValue::from_str(&format!("{STATE_COOKIE}={state}; Path=/api/auth/sso; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}"))
        .expect("cookie")
}

fn pending() -> &'static Mutex<HashMap<String, Pending>> {
    static P: OnceLock<Mutex<HashMap<String, Pending>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

fn random() -> String {
    let mut b = [0u8; 32];
    ring::rand::SystemRandom::new().fill(&mut b).expect("random");
    URL_SAFE_NO_PAD.encode(b)
}

/// Encode en pourcentage tout sauf les caractères non réservés.
pub fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Un chemin local seulement : commence par `/`, ni `//` ni `\`, ni espace ni caractère de contrôle
/// (un navigateur retire une tabulation : `/\t/evil.com` deviendrait `//evil.com`).
pub fn safe_next(next: Option<&str>) -> String {
    match next {
        Some(n) if n.starts_with('/') && !n.contains("//") && !n.contains('\\') && !n.chars().any(|c| c.is_control() || c.is_whitespace()) => n.to_string(),
        _ => "/".into(),
    }
}

fn first_header<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    h.get(name).and_then(|v| v.to_str().ok()).map(|v| v.split(',').next().unwrap_or("").trim()).filter(|v| !v.is_empty())
}

/// Retour à la page de connexion, avec la raison (`?sso_error=…`).
fn fail(code: &str, detail: &str) -> Response {
    if !detail.is_empty() {
        tracing::warn!("SSO sign-in failed ({code}): {detail}");
    }
    Redirect::to(&format!("/?sso_error={code}&sso_detail={}", enc(detail))).into_response()
}

#[derive(Deserialize)]
struct StartQuery {
    next: Option<String>,
}

async fn start(State(s): State<Shared>, headers: HeaderMap, Query(q): Query<StartQuery>) -> Response {
    let cfg = s.config.load().sso.clone();
    if !cfg.active() {
        return fail("failed", "SSO is not set up");
    }
    let issuer = cfg.issuer.clone();
    let disc = match tokio::task::spawn_blocking(move || discover(&issuer, false)).await {
        Ok(Ok(d)) => d,
        Ok(Err(e)) => return fail("failed", &e),
        Err(e) => return fail("failed", &e.to_string()),
    };
    let scheme = if auth::is_https(&headers) { "https" } else { "http" };
    let host = first_header(&headers, "x-forwarded-host").or_else(|| first_header(&headers, "host")).unwrap_or("localhost");
    let (state, nonce, verifier) = (random(), random(), random());
    let challenge = URL_SAFE_NO_PAD.encode(ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes()));
    let redirect_uri = format!("{scheme}://{host}/api/auth/sso/callback");
    let url = format!(
        "{}{}response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&nonce={}&code_challenge={}&code_challenge_method=S256",
        disc.authorization_endpoint,
        if disc.authorization_endpoint.contains('?') { '&' } else { '?' },
        enc(&cfg.client_id),
        enc(&redirect_uri),
        enc("openid profile email"),
        state,
        nonce,
        challenge
    );
    {
        let mut p = pending().lock().unwrap();
        p.retain(|_, v| v.created.elapsed() < PENDING_TTL);
        while p.len() >= PENDING_MAX {
            let Some(old) = p.iter().min_by_key(|(_, v)| v.created).map(|(k, _)| k.clone()) else { break };
            p.remove(&old);
        }
        p.insert(state.clone(), Pending { nonce, verifier, redirect_uri, next: s.config.load().safe_return(q.next.as_deref()), created: Instant::now() });
    }
    let mut res = Redirect::to(&url).into_response();
    res.headers_mut().append(header::SET_COOKIE, state_cookie(&state, &headers));
    res
}

// ---------------------------------------------------------------- retour

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Qui s'est connecté, d'après le jeton d'identité.
#[derive(Debug, PartialEq)]
pub struct Identity {
    /// « émetteur|sujet ».
    pub key: String,
    pub login: String,
    /// Un e-mail vérifié, ou l'upn.
    pub address: Option<String>,
    pub name: String,
    pub groups: Vec<String>,
}

fn claim_str<'a>(c: &'a Value, k: &str) -> Option<&'a str> {
    c.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty())
}

pub fn identity(claims: &Value, groups_claim: &str) -> Result<Identity, String> {
    let iss = claim_str(claims, "iss").ok_or("no issuer in the token")?;
    let sub = claim_str(claims, "sub").ok_or("no subject in the token")?;
    let verified = claims.get("email_verified").and_then(|v| v.as_bool().or_else(|| v.as_str().map(|s| s == "true")));
    let email = claim_str(claims, "email").filter(|_| verified != Some(false));
    let upn = claim_str(claims, "upn");
    let login = email.or(claim_str(claims, "preferred_username")).or(upn).ok_or("the directory sent no user name")?;
    let address = if verified == Some(true) { email } else { None }.or(upn).map(str::to_string);
    let groups = match claims.get(groups_claim) {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    };
    Ok(Identity { key: format!("{iss}|{sub}"), login: login.to_string(), address, name: claim_str(claims, "name").unwrap_or("").to_string(), groups })
}

/// `iss`, `aud`, `exp` (+120 s) et `nonce`.
pub fn check_claims(c: &Value, issuer: &str, client_id: &str, nonce: &str, now: u64) -> Result<(), String> {
    if claim_str(c, "iss").map(|i| i.trim_end_matches('/')) != Some(issuer.trim_end_matches('/')) {
        return Err("the token comes from another issuer".into());
    }
    let aud_ok = match c.get("aud") {
        Some(Value::String(a)) => a == client_id,
        Some(Value::Array(a)) => a.iter().any(|v| v.as_str() == Some(client_id)),
        _ => false,
    };
    if !aud_ok {
        return Err("the token is for another application".into());
    }
    match c.get("exp").and_then(Value::as_u64) {
        Some(exp) if exp + 120 >= now => {}
        _ => return Err("the token has expired".into()),
    }
    if claim_str(c, "nonce") != Some(nonce) {
        return Err("the token does not match this sign-in".into());
    }
    Ok(())
}

fn b64(s: &str) -> Result<Vec<u8>, String> {
    URL_SAFE_NO_PAD.decode(s.trim_end_matches('=')).map_err(|_| "bad base64 in the token".to_string())
}

fn verify_signature(alg: &str, key: &Value, msg: &[u8], sig: &[u8]) -> bool {
    use ring::signature;
    let field = |k: &str| key.get(k).and_then(Value::as_str).and_then(|v| b64(v).ok());
    match alg {
        "RS256" | "PS256" => {
            let (Some(n), Some(e)) = (field("n"), field("e")) else { return false };
            let params: &signature::RsaParameters = if alg == "RS256" { &signature::RSA_PKCS1_2048_8192_SHA256 } else { &signature::RSA_PSS_2048_8192_SHA256 };
            signature::RsaPublicKeyComponents { n, e }.verify(params, msg, sig).is_ok()
        }
        "ES256" => {
            let (Some(x), Some(y)) = (field("x"), field("y")) else { return false };
            let mut point = vec![4u8];
            point.extend(x);
            point.extend(y);
            signature::UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_FIXED, point).verify(msg, sig).is_ok()
        }
        _ => false,
    }
}

/// Vérifie la signature du jeton avec les clés du fournisseur (rechargées une fois si la clé est inconnue).
fn verified_claims(token: &str, jwks_uri: &str) -> Result<Value, String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err("the id token is malformed".into());
    }
    let head: Value = serde_json::from_slice(&b64(parts[0])?).map_err(|_| "the id token is malformed")?;
    let alg = head.get("alg").and_then(Value::as_str).unwrap_or("");
    if !matches!(alg, "RS256" | "PS256" | "ES256") {
        return Err(format!("unsupported token signature {alg}"));
    }
    let kid = head.get("kid").and_then(Value::as_str);
    let msg = format!("{}.{}", parts[0], parts[1]);
    let sig = b64(parts[2])?;
    let want_kty = if alg == "ES256" { "EC" } else { "RSA" };
    for fresh in [false, true] {
        let keys = jwks(jwks_uri, fresh)?;
        let candidates: Vec<&Value> = keys
            .iter()
            .filter(|k| k.get("kty").and_then(Value::as_str) == Some(want_kty))
            .filter(|k| kid.is_none() || k.get("kid").and_then(Value::as_str) == kid)
            .collect();
        if candidates.is_empty() {
            continue;
        }
        if candidates.iter().any(|k| verify_signature(alg, k, msg.as_bytes(), &sig)) {
            return serde_json::from_slice(&b64(parts[1])?).map_err(|_| "the id token is malformed".to_string());
        }
        return Err("the token signature is wrong".into());
    }
    Err("the token is signed with an unknown key".into())
}

fn exchange(cfg: &SsoConfig, disc: &Discovery, code: &str, p: &Pending) -> Result<String, String> {
    let mut form =
        vec![("grant_type", "authorization_code"), ("code", code), ("redirect_uri", p.redirect_uri.as_str()), ("code_verifier", p.verifier.as_str())];
    let mut req = agent().post(&disc.token_endpoint).header("Accept", "application/json");
    if cfg.client_secret.is_empty() {
        form.push(("client_id", cfg.client_id.as_str()));
    } else {
        let basic = base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", enc(&cfg.client_id), enc(&cfg.client_secret)));
        req = req.header("Authorization", &format!("Basic {basic}"));
    }
    let mut res = req.send_form(form).map_err(|e| e.to_string())?;
    let body: Value = res.body_mut().read_to_string().ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    if !res.status().is_success() {
        let why = claim_str(&body, "error_description")
            .or(claim_str(&body, "error"))
            .map(str::to_string)
            .unwrap_or_else(|| format!("HTTP {}", res.status().as_u16()));
        return Err(why);
    }
    claim_str(&body, "id_token").map(str::to_string).ok_or_else(|| "the directory sent no id token".into())
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

async fn callback(State(s): State<Shared>, headers: HeaderMap, Query(q): Query<CallbackQuery>) -> Response {
    if let Some(e) = q.error.as_deref() {
        return fail("failed", q.error_description.as_deref().unwrap_or(e));
    }
    // L'état donné à ce navigateur au départ : un lien de retour envoyé d'ailleurs ne le connecte pas.
    let mine = auth::cookie(&headers, STATE_COOKIE);
    let p = q.state.as_deref().filter(|st| mine.as_deref() == Some(*st)).and_then(|st| pending().lock().unwrap().remove(st));
    let Some(p) = p.filter(|p| p.created.elapsed() < PENDING_TTL) else { return fail("expired", "") };
    let Some(code) = q.code else { return fail("failed", "no code") };
    let cfg = s.config.load().sso.clone();
    if !cfg.active() {
        return fail("failed", "SSO is not set up");
    }
    let c = cfg.clone();
    let next = p.next.clone();
    let got = tokio::task::spawn_blocking(move || -> Result<Identity, String> {
        let disc = discover(&c.issuer, false)?;
        let token = exchange(&c, &disc, &code, &p)?;
        let claims = verified_claims(&token, &disc.jwks_uri)?;
        check_claims(&claims, &disc.issuer, &c.client_id, &p.nonce, now_secs())?;
        identity(&claims, &c.groups_claim)
    })
    .await;
    let id = match got {
        Ok(Ok(id)) => id,
        Ok(Err(e)) => return fail("failed", &e),
        Err(e) => return fail("failed", &e.to_string()),
    };
    if !cfg.domain_allowed(&id) {
        tracing::warn!("sign-in refused with {}: {} is not in an allowed domain", cfg.button_name(), id.login);
        return fail("domain", "");
    }
    let admin = cfg.is_admin(&id.groups);
    let groups = s.groups.from_directory(&id.groups);
    if !admin && groups.is_empty() && !cfg.allow_any {
        tracing::warn!("sign-in refused with {}: {} is in no known group", cfg.button_name(), id.login);
        return fail("noGroup", "");
    }
    let auth = s.auth.clone();
    let user = match blocking(move || sign_in_account(&auth, &id, admin, groups)).await {
        Ok(Ok(u)) => u,
        Ok(Err(e)) if e == "disabled" => return fail("disabled", ""),
        Ok(Err(e)) => return fail("failed", &e),
        Err((_, e)) => return fail("failed", &e),
    };
    tracing::info!("{} signed in with {}", user.username, cfg.button_name());
    let token = s.auth.open_session(&user.id);
    let domain = s.config.load().cookie_domain(auth::host(&headers));
    let mut res = Redirect::to(&next).into_response();
    res.headers_mut().insert(header::SET_COOKIE, auth::session_cookie(&token, &headers, domain.as_deref()));
    res.headers_mut().append(header::SET_COOKIE, state_cookie("", &headers));
    res
}

/// Un nom d'utilisateur tiré de l'identifiant de l'annuaire : caractères permis, unique (`-2`, `-3`…).
pub(crate) fn free_username(login: &str, list: &[User]) -> String {
    let mut base: String = login.chars().filter(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '@')).take(60).collect();
    if base.is_empty() {
        base = "user".into();
    }
    let taken = |n: &str| list.iter().any(|u| u.username.to_lowercase() == n.to_lowercase());
    if !taken(&base) {
        return base;
    }
    (2..).map(|i| format!("{base}-{i}")).find(|n| !taken(n)).unwrap()
}

/// Trouve (par la clé de l'annuaire, ou un compte local nommé d'après l'adresse garantie) ou crée le
/// compte ; ses groupes viennent de l'annuaire, son rôle aussi sauf s'il est fixé à la main. L'annuaire
/// ne retire jamais le dernier administrateur.
pub fn sign_in_account(a: &auth::Auth, id: &Identity, admin: bool, groups: Vec<String>) -> Result<User, String> {
    a.edit(|list| {
        let found = list.iter().position(|u| u.sso == id.key).or_else(|| {
            id.address
                .as_deref()
                .and_then(|addr| list.iter().position(|u| u.sso.is_empty() && u.username.contains('@') && u.username.eq_ignore_ascii_case(addr)))
        });
        match found {
            Some(i) => {
                if list[i].disabled {
                    return Err("disabled".into());
                }
                list[i].sso = id.key.clone();
                list[i].groups = groups;
                if !list[i].admin_fixed {
                    let was = list[i].admin;
                    list[i].admin = admin;
                    if !auth::has_active_admin(list) {
                        list[i].admin = was;
                    }
                }
                Ok(list[i].clone())
            }
            None => {
                let u = User::from_directory(free_username(&id.login, list), id.name.clone(), admin, groups, id.key.clone());
                list.push(u.clone());
                Ok(u)
            }
        }
    })
}

// ---------------------------------------------------------------- réglages

/// Les réglages, secret masqué, et l'adresse de retour à déclarer chez le fournisseur.
async fn read(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let cfg = s.config.load();
    let mut sso = serde_json::to_value(&cfg.sso).unwrap_or(Value::Null);
    if !cfg.sso.client_secret.is_empty() {
        sso["clientSecret"] = json!(MASK);
    }
    Ok(Json(json!({"sso": sso, "redirectUri": cfg.sso_redirect_uri(), "buttonName": cfg.sso.button_name()})))
}

/// Enregistre les réglages ; le secret masqué garde l'ancien.
async fn write(State(s): State<Shared>, CurrentUser(me): CurrentUser, Json(mut b): Json<SsoConfig>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    b.normalize();
    if b.enabled && (b.issuer.is_empty() || b.client_id.is_empty()) {
        return Err(bad("the issuer and the client id are needed"));
    }
    // En clair, n'importe qui sur le chemin fournirait ses propres clés : https, sauf sur cette
    // machine.
    let local = ["http://localhost", "http://127.0.0.1", "http://[::1]"]
        .iter()
        .any(|p| b.issuer == *p || b.issuer.starts_with(&format!("{p}:")) || b.issuer.starts_with(&format!("{p}/")));
    if b.enabled && !(b.issuer.starts_with("https://") || local) {
        return Err(bad("the issuer is an https:// address"));
    }
    let s2 = s.clone();
    blocking(move || {
        s2.update_config(|cfg| {
            if b.client_secret == MASK {
                b.client_secret = cfg.sso.client_secret.clone();
            }
            cfg.sso = b;
            Ok(())
        })
    })
    .await?
    .map_err(bad)?;
    tracing::info!("SSO settings changed by {}", me.username);
    read(State(s), CurrentUser(me)).await
}

#[derive(Deserialize)]
struct TestBody {
    #[serde(default)]
    issuer: String,
}

/// Teste un émetteur : sa découverte et ses clés répondent-elles ?
async fn test(CurrentUser(me): CurrentUser, Json(b): Json<TestBody>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let issuer = b.issuer.trim().trim_end_matches('/').to_string();
    if issuer.is_empty() {
        return Err(bad("the issuer is empty"));
    }
    let r = blocking(move || -> Result<Value, String> {
        let d = discover(&issuer, true)?;
        let keys = jwks(&d.jwks_uri, true)?;
        Ok(json!({"issuer": d.issuer, "authorizationEndpoint": d.authorization_endpoint, "tokenEndpoint": d.token_endpoint, "keys": keys.len()}))
    })
    .await?;
    r.map(Json).map_err(|e| (StatusCode::BAD_GATEWAY, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn config() {
        let mut c: SsoConfig = serde_json::from_value(json!({"enabled": true, "provider": "Okta", "issuer": " https://id.example.com/ ", "clientId": "abc", "adminGroups": [" ", "/IT-Admins"], "domains": ["@Example.org"]})).unwrap();
        c.normalize();
        assert_eq!(c.issuer, "https://id.example.com");
        assert_eq!(c.admin_groups, vec!["/IT-Admins".to_string()]);
        assert_eq!(c.domains, vec!["example.org".to_string()]);
        assert!(c.active());
        assert_eq!(c.button_name(), "Okta");
        assert_eq!(c.local_accounts, LocalAccounts::AdminOnly);
        assert!(c.local_allowed(true) && !c.local_allowed(false));
        assert!(c.is_admin(&["it-admins".into()]));
        assert!(!c.is_admin(&["other".into()]));
        assert_eq!(serde_json::to_value(LocalAccounts::Off).unwrap(), json!("none"));
        assert!(SsoConfig::default().local_allowed(false), "off: everyone");
        assert_eq!(SsoConfig::default().local_accounts_now(), LocalAccounts::All);
        let who = |address: Option<&str>| Identity {
            key: "k".into(),
            login: "x".into(),
            address: address.map(String::from),
            name: String::new(),
            groups: Vec::new(),
        };
        assert!(c.domain_allowed(&who(Some("a@example.ORG"))));
        assert!(!c.domain_allowed(&who(Some("a@example.org.evil.com"))));
        assert!(!c.domain_allowed(&who(None)), "no verified address");
        let google = SsoConfig { provider: "google".into(), ..Default::default() };
        assert!(!google.domain_allowed(&who(Some("a@gmail.com"))), "Google needs its domains");
        assert!(SsoConfig::default().domain_allowed(&who(Some("a@anything.example"))), "no list, not Google: any");
    }

    #[test]
    fn claims_and_identity() {
        let c = json!({"iss": "https://id.example.com/", "aud": ["x", "app"], "exp": 1000, "nonce": "n1", "sub": "42",
            "email": "Ann@Example.com", "email_verified": true, "name": "Ann", "groups": ["/Realisateurs"]});
        assert!(check_claims(&c, "https://id.example.com", "app", "n1", 1100).is_ok());
        assert!(check_claims(&c, "https://id.example.com", "app", "n1", 1121).is_err(), "expired");
        assert!(check_claims(&c, "https://other", "app", "n1", 900).is_err());
        assert!(check_claims(&c, "https://id.example.com", "other", "n1", 900).is_err());
        assert!(check_claims(&c, "https://id.example.com", "app", "n2", 900).is_err());
        let id = identity(&c, "groups").unwrap();
        assert_eq!(id.key, "https://id.example.com/|42");
        assert_eq!(id.login, "Ann@Example.com");
        assert_eq!(id.address.as_deref(), Some("Ann@Example.com"));
        assert_eq!(id.groups, vec!["/Realisateurs".to_string()]);
        let entra = json!({"iss": "i", "sub": "s", "email": "a@b.c", "upn": "a@corp.c", "preferred_username": "a@corp.c"});
        let id = identity(&entra, "groups").unwrap();
        assert_eq!(id.login, "a@b.c");
        assert_eq!(id.address.as_deref(), Some("a@corp.c"), "email not verified: upn");
        let unverified = json!({"iss": "i", "sub": "s", "email": "a@b.c", "email_verified": false, "preferred_username": "ann"});
        assert_eq!(identity(&unverified, "groups").unwrap().login, "ann");
    }

    #[test]
    fn next_and_names() {
        assert_eq!(safe_next(Some("/reglages/utilisateurs")), "/reglages/utilisateurs");
        assert_eq!(safe_next(Some("//evil.com")), "/");
        assert_eq!(safe_next(Some("/\\evil.com")), "/");
        assert_eq!(safe_next(Some("https://evil.com")), "/");
        assert_eq!(safe_next(Some("/\t/evil.com")), "/");
        assert_eq!(safe_next(None), "/");
        assert_eq!(enc("a b/é"), "a%20b%2F%C3%A9");
        let list = vec![
            User::from_directory("ann".into(), String::new(), false, vec![], String::new()),
            User::from_directory("ann-2".into(), String::new(), false, vec![], String::new()),
        ];
        assert_eq!(free_username("Ann", &list), "Ann-3");
        assert_eq!(free_username("bob smith", &list), "bobsmith");
    }
}
