//! Réglages › Ce serveur : son nom et ses adresses, le HTTPS, le redémarrage du Core (administrateurs).
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{net::IpAddr, time::Duration};

use crate::{
    api::{bad, blocking, require, ApiResult},
    auth::{host, CurrentUser},
    config::{hostname, HttpsMode, ServerConfig},
    state::Shared,
    tls,
};

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/server", get(read).put(write))
        .route("/api/server/dns", get(dns))
        .route("/api/server/restart", post(restart))
        .route("/api/https", get(https).put(set_https))
        .route("/api/https/import", post(import))
        .route("/api/https/ca.crt", get(ca))
}

/// Un nom DNS : des mots de lettres, chiffres et tirets, séparés par des points (deux au moins).
pub fn valid_dns(name: &str) -> Result<(), String> {
    let labels: Vec<&str> = name.split('.').collect();
    let ok_label = |l: &&str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    };
    if name.len() <= 200 && labels.len() >= 2 && labels.iter().all(ok_label) {
        Ok(())
    } else {
        Err(format!("not a DNS name: {name}"))
    }
}

/// Un fuseau horaire IANA (`Europe/Paris`, `UTC`).
fn valid_timezone(tz: &str) -> bool {
    !tz.is_empty() && tz.len() <= 64 && tz.split('/').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || "_-+".contains(c)))
}

/// Les adresses du serveur vues depuis une requête : le cockpit, les réglages, chaque appli active.
fn addresses(s: &Shared, cfg: &ServerConfig, at: Option<&str>) -> Vec<Value> {
    let core = cfg.core_url_at(at);
    let mut out = vec![json!({"label": "Cockpit", "url": core}), json!({"label": "Settings", "url": format!("{core}reglages")})];
    for a in s.apps.list().into_iter().filter(|a| a.active) {
        out.push(json!({"label": a.manifest.name, "url": cfg.app_url_at(&a.manifest.id, at), "app": a.manifest.id}));
    }
    out
}

async fn read(State(s): State<Shared>, CurrentUser(me): CurrentUser, headers: HeaderMap) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let cfg = s.config.load();
    let apps = s.apps.list();
    Ok(Json(json!({
        "name": cfg.name,
        "dnsName": cfg.dns_name,
        "timezone": cfg.timezone,
        "hostname": hostname(),
        "version": strata_core::VERSION,
        "uptimeSecs": s.started_at.elapsed().as_secs(),
        "http": s.tls.http_addr().map(|a| a.to_string()),
        "https": {"mode": cfg.https.mode, "port": cfg.https.port, "listening": s.tls.listening()},
        "scheme": cfg.scheme(),
        "addresses": addresses(&s, &cfg, host(&headers)),
        "apps": {"installed": apps.len(), "active": apps.iter().filter(|a| a.active).count()},
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerBody {
    name: String,
    dns_name: String,
    timezone: String,
}

/// Le nom, le nom DNS (les adresses, le certificat fait par Strata) et le fuseau horaire.
async fn write(State(s): State<Shared>, CurrentUser(me): CurrentUser, headers: HeaderMap, Json(b): Json<ServerBody>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let name = b.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(bad("the server needs a name (64 characters at most)"));
    }
    let dns = b.dns_name.trim().trim_end_matches('.').to_lowercase();
    valid_dns(&dns).map_err(bad)?;
    if !valid_timezone(b.timezone.trim()) {
        return Err(bad(format!("not a time zone: {}", b.timezone)));
    }
    let tz = b.timezone.trim().to_string();
    let renamed = update(&s, move |cfg| {
        let renamed = cfg.dns_name != dns;
        cfg.name = name;
        cfg.dns_name = dns;
        cfg.timezone = tz;
        Ok(renamed)
    })
    .await?;
    tracing::info!("server settings changed by {}", me.username);
    if renamed {
        // Le certificat fait par Strata suit le nouveau nom.
        tls::apply(&s).await;
    }
    read(State(s), CurrentUser(me), headers).await
}

/// Change les réglages du serveur sous le verrou d'écriture (`AppState::update_config`).
async fn update<R: Send + 'static>(s: &Shared, f: impl FnOnce(&mut ServerConfig) -> Result<R, String> + Send + 'static) -> ApiResult<R> {
    let s2 = s.clone();
    blocking(move || s2.update_config(f)).await?.map_err(bad)
}

/// Le nom DNS et ses sous-domaines mènent-ils à ce serveur ?
async fn dns(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let name = s.config.load().dns_name.clone();
    let local: Vec<IpAddr> = if_addrs::get_if_addrs().unwrap_or_default().into_iter().filter(|i| !i.is_loopback()).map(|i| i.ip()).collect();
    let look = |n: String| async move {
        match tokio::time::timeout(Duration::from_secs(3), tokio::net::lookup_host((n.as_str(), 0))).await {
            Ok(Ok(addrs)) => {
                let mut ips: Vec<IpAddr> = addrs.map(|a| a.ip()).collect();
                ips.sort();
                ips.dedup();
                ips
            }
            _ => Vec::new(),
        }
    };
    let base = look(name.clone()).await;
    let wildcard = look(format!("strata-check.{name}")).await;
    let here = |ips: &[IpAddr]| ips.iter().any(|ip| ip.is_loopback() || local.contains(ip));
    Ok(Json(json!({
        "name": name,
        "base": base,
        "baseHere": here(&base),
        "wildcard": wildcard,
        "wildcardHere": here(&wildcard),
        "local": local,
    })))
}

/// Redémarre le Core (les applis qu'il lance redémarrent avec lui).
async fn restart(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<StatusCode> {
    require(me.admin)?;
    tracing::info!("restart asked by {}", me.username);
    let s2 = s.clone();
    tokio::spawn(async move {
        // Le temps de répondre.
        tokio::time::sleep(Duration::from_millis(300)).await;
        s2.ask_restart();
    });
    Ok(StatusCode::ACCEPTED)
}

async fn https_state(s: &Shared) -> Value {
    let cfg = s.config.load();
    json!({
        "mode": cfg.https.mode,
        "port": cfg.https.port,
        "listening": s.tls.listening(),
        "cert": s.tls.info(),
        "company": s.tls.company(&cfg.dns_name),
        "ca": s.tls.ca_pem().is_some(),
        "names": tls::needed_names(&cfg.dns_name),
        "http": s.tls.http_addr().map(|a| a.port()),
    })
}

async fn https(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    Ok(Json(https_state(&s).await))
}

#[derive(Deserialize)]
struct HttpsBody {
    mode: HttpsMode,
    port: u16,
}

/// Change l'origine du HTTPS ou son port ; l'écoute suit aussitôt.
async fn set_https(State(s): State<Shared>, CurrentUser(me): CurrentUser, Json(b): Json<HttpsBody>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    if b.port == 0 {
        return Err(bad("the HTTPS port cannot be 0"));
    }
    if s.tls.http_addr().is_some_and(|a| a.port() == b.port) {
        return Err(bad(format!("port {} is already the HTTP one", b.port)));
    }
    if b.mode == HttpsMode::Company && s.tls.company(&s.config.load().dns_name).is_none() {
        return Err(bad("import the company certificate first"));
    }
    let (mode, port) = (b.mode, b.port);
    update(&s, move |cfg| {
        cfg.https.mode = mode;
        cfg.https.port = port;
        Ok(())
    })
    .await?;
    tracing::info!("HTTPS set to {:?} on port {} by {}", b.mode, b.port, me.username);
    tls::apply(&s).await;
    Ok(Json(https_state(&s).await))
}

#[derive(Deserialize)]
struct ImportBody {
    /// La chaîne de certificats, en PEM (le certificat du serveur d'abord).
    cert: String,
    /// La clé privée, en PEM.
    key: String,
}

/// Importe le certificat de l'entreprise (sans le mettre en service : c'est le choix de l'origine).
async fn import(State(s): State<Shared>, CurrentUser(me): CurrentUser, Json(b): Json<ImportBody>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let dns = s.config.load().dns_name.clone();
    let dir = s.data_dir.join("tls");
    blocking(move || tls::import(&dir, &b.cert, &b.key, &dns)).await?.map_err(bad)?;
    tracing::info!("company certificate imported by {}", me.username);
    if s.config.load().https.mode == HttpsMode::Company {
        tls::apply(&s).await;
    }
    Ok(Json(https_state(&s).await))
}

/// L'autorité de ce serveur, à installer sur les postes : publique, comme tout certificat.
async fn ca(State(s): State<Shared>) -> Response {
    let Some(pem) = s.tls.ca_pem() else { return (StatusCode::NOT_FOUND, "no authority yet").into_response() };
    let file = format!("attachment; filename=\"strata-{}-ca.crt\"", hostname());
    ([(header::CONTENT_TYPE, "application/x-x509-ca-cert".to_string()), (header::CONTENT_DISPOSITION, file)], pem).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_names_and_zones() {
        assert!(valid_dns("strata01.regie.exemple.fr").is_ok());
        assert!(valid_dns("strata01.local").is_ok());
        assert!(valid_dns("strata01").is_err(), "one word: no cookie for the apps");
        assert!(valid_dns("Strata01.local").is_err());
        assert!(valid_dns("-a.local").is_err());
        assert!(valid_dns("a..local").is_err());
        assert!(valid_timezone("Europe/Paris") && valid_timezone("UTC") && valid_timezone("America/Argentina/Buenos_Aires") && valid_timezone("Etc/GMT+2"));
        assert!(!valid_timezone("../etc") && !valid_timezone("") && !valid_timezone("Europe/Paris "));
    }
}
