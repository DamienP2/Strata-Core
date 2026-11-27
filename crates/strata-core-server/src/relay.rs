//! Le relais vers les applis : une requête pour `<appli>.<nom du serveur>` part vers le port interne
//! de l'appli (127.0.0.1), WebSocket compris. Le Core est la seule porte d'entrée (brief § 4).
//!
//! - Il faut être connecté au Core (la session vaut pour tous les sous-domaines) ; sans session, une
//!   page demandée renvoie à la connexion du cockpit, qui ramène ensuite à l'appli ; une requête de
//!   l'appli (API, WebSocket) reçoit 401. Les chemins `[relay] public` de la fiche passent sans session.
//! - L'appli apprend qui est connecté par les en-têtes `X-Strata-*` ; ceux envoyés par le navigateur
//!   sont retirés, et le cookie de session du Core ne lui est pas transmis. Ses droits, l'appli les
//!   décide elle-même. Le contrat complet : docs/APPLIS.md.
//! - Les réponses de l'appli sont rendues telles quelles (ses propres en-têtes de sécurité).
use axum::{
    body::Body,
    extract::{ConnectInfo, Request, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, Version},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::{TokioExecutor, TokioIo},
};
use std::{net::SocketAddr, time::Duration};
use strata_core::manifest::valid_id;

use crate::{
    apps::Installed,
    auth::{self, User},
    config::ServerConfig,
    guard,
    sso::enc,
    state::Shared,
    supervisor::RunState,
};

pub type HttpClient = Client<HttpConnector, Body>;

pub fn client() -> HttpClient {
    let mut c = HttpConnector::new();
    c.set_nodelay(true);
    c.set_connect_timeout(Some(Duration::from_secs(2)));
    Client::builder(TokioExecutor::new()).pool_idle_timeout(Duration::from_secs(30)).build(c)
}

/// L'appli demandée par le nom : `livecut.strata01.regie.fr[:8080]` → `livecut`.
pub fn app_of_host(dns_name: &str, host: &str) -> Option<String> {
    let name = host.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map_or(host, |(h, _)| h).to_ascii_lowercase();
    let sub = name.strip_suffix(dns_name)?.strip_suffix('.')?;
    valid_id(sub).ok()?;
    Some(sub.to_string())
}

/// Le relais, avant tout le reste : une requête pour une appli ne voit jamais l'API du Core.
pub async fn relay(State(s): State<Shared>, req: Request, next: Next) -> Response {
    let cfg = s.config.load();
    let Some(id) = auth::host(req.headers()).and_then(|h| app_of_host(&cfg.dns_name, h)) else {
        return next.run(req).await;
    };
    let token = auth::token_from(req.headers());
    let user = token.as_deref().and_then(|t| s.auth.user_for_token(t)).filter(|u| !u.disabled);
    let fr = speaks_french(user.as_ref(), req.headers());
    let back = cfg.core_url_at(auth::host(req.headers()));
    let Some(app) = s.apps.get(&id).filter(|a| a.active && a.compatible) else {
        let text = if fr { "Cette appli n’est pas disponible sur ce serveur." } else { "This app is not available on this server." };
        return answer(&req, StatusCode::NOT_FOUND, fr, &id, text, &back, false);
    };
    let public = app.manifest.relay.is_public(req.uri().path());
    if user.is_none() && !public {
        return sign_in_first(&cfg, &req);
    }
    // Tout changement de protocole est vérifié comme un WebSocket.
    let upgrading =
        req.headers().contains_key(header::UPGRADE) && !req.headers().get(header::AUTHORIZATION).is_some_and(|v| v.as_bytes().starts_with(b"Bearer "));
    if (upgrading || guard::needs_check(req.method(), req.headers())) && guard::other_site(req.headers(), &cfg.trusted_origins) {
        return (StatusCode::FORBIDDEN, "request sent by another site refused").into_response();
    }
    // Une adresse publique ne dit pas qui est connecté.
    let user = if public { None } else { user };
    forward(&s, &app, user.as_ref(), token.as_deref(), req, fr, &back).await
}

/// Sans session : une page renvoie à la connexion du cockpit (qui ramène ici), le reste reçoit 401.
fn sign_in_first(cfg: &ServerConfig, req: &Request) -> Response {
    if !wants_page(req) {
        return (StatusCode::UNAUTHORIZED, "login required").into_response();
    }
    let host = auth::host(req.headers()).unwrap_or_default();
    let here = format!("{}://{host}{}", cfg.scheme(), req.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/"));
    Redirect::to(&format!("{}?next={}", cfg.core_url_at(Some(host)), enc(&here))).into_response()
}

/// Une page demandée par le navigateur (et pas une requête de son code ou un WebSocket) ?
fn wants_page(req: &Request) -> bool {
    let h = req.headers();
    matches!(*req.method(), Method::GET | Method::HEAD)
        && !h.contains_key(header::UPGRADE)
        && h.get(header::ACCEPT).and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("text/html"))
}

fn speaks_french(user: Option<&User>, h: &HeaderMap) -> bool {
    match user.map(|u| u.lang.as_str()) {
        Some("fr") => true,
        Some("en") => false,
        _ => h.get(header::ACCEPT_LANGUAGE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.trim_start().to_ascii_lowercase().starts_with("fr")),
    }
}

/// Les en-têtes propres à une connexion, jamais relayés.
const HOP: [&str; 8] = ["connection", "keep-alive", "proxy-connection", "proxy-authenticate", "proxy-authorization", "te", "trailer", "transfer-encoding"];

fn drop_hop_headers(h: &mut HeaderMap) {
    let named: Vec<HeaderName> = h
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|n| HeaderName::from_bytes(n.trim().as_bytes()).ok())
        .collect();
    for n in named {
        h.remove(n);
    }
    for n in HOP {
        h.remove(n);
    }
    h.remove(header::UPGRADE);
}

/// Les cookies envoyés à l'appli : les siens, sans la session du Core.
fn without_core_cookie(h: &mut HeaderMap) {
    let kept: Vec<String> = h
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .map(str::trim)
        .filter(|c| !c.is_empty() && !c.starts_with(&format!("{}=", auth::COOKIE)))
        .map(str::to_string)
        .collect();
    h.remove(header::COOKIE);
    if let Ok(v) = HeaderValue::from_str(&kept.join("; ")) {
        if !kept.is_empty() {
            h.insert(header::COOKIE, v);
        }
    }
}

fn set(h: &mut HeaderMap, name: &'static str, value: &str) {
    if let Ok(v) = HeaderValue::from_str(value) {
        h.insert(name, v);
    }
}

/// Qui est connecté, pour l'appli (texte libre encodé en pourcentage).
fn identity(s: &Shared, h: &mut HeaderMap, u: &User) {
    set(h, "x-strata-user-id", &u.id);
    set(h, "x-strata-user", &enc(&u.username));
    set(h, "x-strata-user-name", &enc(u.name()));
    set(h, "x-strata-admin", if u.admin { "1" } else { "0" });
    set(h, "x-strata-groups", &u.groups.join(","));
    set(h, "x-strata-group-names", &s.groups.names_of(&u.groups).iter().map(|n| enc(n)).collect::<Vec<_>>().join(","));
    set(h, "x-strata-lang", &u.lang);
    set(h, "x-strata-theme", &u.theme);
}

async fn forward(s: &Shared, app: &Installed, user: Option<&User>, token: Option<&str>, mut req: Request, fr: bool, back: &str) -> Response {
    let port = app.manifest.port;
    let page = wants_page(&req);
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
    let upgrade = req.headers().get(header::UPGRADE).cloned();
    let client_side = upgrade.is_some().then(|| hyper::upgrade::on(&mut req));
    let (mut parts, body) = req.into_parts();
    let h = &mut parts.headers;
    let proto = if auth::is_https(h) { "https" } else { "http" };
    let host = auth::host(h).map(str::to_string);
    drop_hop_headers(h);
    without_core_cookie(h);
    if let Some(t) = token {
        if h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).is_some_and(|v| v.strip_prefix("Bearer ").is_some_and(|b| b.trim() == t)) {
            h.remove(header::AUTHORIZATION);
        }
    }
    let theirs: Vec<HeaderName> = h.keys().filter(|k| k.as_str().starts_with("x-strata-")).cloned().collect();
    for k in theirs {
        h.remove(k);
    }
    if let Some(u) = user {
        identity(s, h, u);
    }
    if let Some(peer) = peer {
        let ip = guard::plain_ip(peer.ip()).to_string();
        // Derrière un relais de cette machine, la chaîne qu'il a commencée continue.
        let chain = match h.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            Some(before) if guard::plain_ip(peer.ip()).is_loopback() => format!("{before}, {ip}"),
            _ => ip,
        };
        set(h, "x-forwarded-for", &chain);
    }
    set(h, "x-forwarded-proto", proto);
    if let Some(host) = &host {
        set(h, "x-forwarded-host", host);
    }
    if let Some(up) = upgrade {
        h.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
        h.insert(header::UPGRADE, up);
    }
    let pq = parts.uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    parts.uri = match format!("http://127.0.0.1:{port}{pq}").parse::<Uri>() {
        Ok(u) => u,
        Err(_) => return (StatusCode::BAD_REQUEST, "bad address").into_response(),
    };
    parts.version = Version::HTTP_11;
    parts.extensions = Default::default();

    let mut res = match s.relay.request(Request::from_parts(parts, body)).await {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!("{}: {e}", app.manifest.id);
            return down(s, app, page, fr, back);
        }
    };
    if res.status() == StatusCode::SWITCHING_PROTOCOLS {
        if let Some(client_side) = client_side {
            let app_side = hyper::upgrade::on(&mut res);
            let id = app.manifest.id.clone();
            tokio::spawn(async move {
                match (client_side.await, app_side.await) {
                    (Ok(a), Ok(b)) => {
                        let _ = tokio::io::copy_bidirectional(&mut TokioIo::new(a), &mut TokioIo::new(b)).await;
                    }
                    (Err(e), _) | (_, Err(e)) => tracing::debug!("{id}: upgrade failed: {e}"),
                }
            });
        }
        let (parts, _) = res.into_parts();
        return Response::from_parts(parts, Body::empty());
    }
    let (mut parts, body) = res.into_parts();
    drop_hop_headers(&mut parts.headers);
    Response::from_parts(parts, Body::new(body))
}

/// L'appli ne répond pas : elle démarre, elle a planté (le Core la relance), ou elle est arrêtée.
fn down(s: &Shared, app: &Installed, page: bool, fr: bool, back: &str) -> Response {
    let name = &app.manifest.name;
    let state = if app.manifest.run.is_some() { s.supervisor.status(&app.manifest.id).state } else { RunState::Stopped };
    let text = match (state, fr) {
        (RunState::Starting, true) => format!("{name} démarre…"),
        (RunState::Starting, false) => format!("{name} is starting…"),
        (RunState::Crashed, true) => format!("{name} s’est arrêtée ; le serveur la relance."),
        (RunState::Crashed, false) => format!("{name} stopped; the server is restarting it."),
        (_, true) => format!("{name} ne répond pas."),
        (_, false) => format!("{name} does not answer."),
    };
    if !page {
        return (StatusCode::BAD_GATEWAY, text).into_response();
    }
    page_response(StatusCode::BAD_GATEWAY, fr, name, &text, back, true)
}

fn answer(req: &Request, status: StatusCode, fr: bool, title: &str, text: &str, back: &str, refresh: bool) -> Response {
    if !wants_page(req) {
        return (status, text.to_string()).into_response();
    }
    page_response(status, fr, title, text, back, refresh)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Une page simple aux couleurs de Strata, sans script ; `refresh` : se recharge toutes les 3 s.
fn page_response(status: StatusCode, fr: bool, title: &str, text: &str, back: &str, refresh: bool) -> Response {
    let (lang, cockpit) = if fr { ("fr", "Retour au cockpit") } else { ("en", "Back to the cockpit") };
    let refresh = if refresh { r#"<meta http-equiv="refresh" content="3">"# } else { "" };
    let html = format!(
        r#"<!doctype html><html lang="{lang}"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">{refresh}<title>{title} · Strata</title><style>
:root{{color-scheme:dark;--bg:#0e1014;--panel:#161a21;--line:#262c36;--text:#e6e9ef;--muted:#8b93a3;--accent:#8b9cf7}}
@media (prefers-color-scheme:light){{:root{{color-scheme:light;--bg:#f4f5f8;--panel:#fff;--line:#e1e4ea;--text:#1b1f27;--muted:#5d6676;--accent:#5b6ee1}}}}
body{{margin:0;min-height:100vh;display:grid;place-items:center;background:var(--bg);color:var(--text);font:14px/1.5 Inter,system-ui,sans-serif}}
main{{background:var(--panel);border:1px solid var(--line);border-radius:12px;padding:28px 32px;max-width:420px;margin:16px;text-align:center}}
h1{{font-size:16px;margin:0 0 6px}}p{{margin:0 0 18px;color:var(--muted)}}a{{color:var(--accent);text-decoration:none;font-weight:500}}
</style></head><body><main><h1>{title}</h1><p>{text}</p><a href="{back}">{cockpit}</a></main></body></html>"#,
        title = esc(title),
        text = esc(text),
        back = esc(back),
    );
    let mut res = (status, html).into_response();
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_from_the_name() {
        let d = "strata01.regie.fr";
        assert_eq!(app_of_host(d, "livecut.strata01.regie.fr").as_deref(), Some("livecut"));
        assert_eq!(app_of_host(d, "LiveCut.Strata01.Regie.fr:8080").as_deref(), Some("livecut"));
        assert_eq!(app_of_host(d, "strata01.regie.fr"), None, "the cockpit");
        assert_eq!(app_of_host(d, "a.b.strata01.regie.fr"), None, "one level only");
        assert_eq!(app_of_host(d, "livecutstrata01.regie.fr"), None);
        assert_eq!(app_of_host(d, "10.0.0.20:8080"), None);
    }

    #[test]
    fn cookies_and_hop_headers() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("strata_session=abc; theme=dark; app_sid=42"));
        without_core_cookie(&mut h);
        assert_eq!(h.get(header::COOKIE).unwrap(), "theme=dark; app_sid=42");
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("strata_session=abc"));
        without_core_cookie(&mut h);
        assert!(h.get(header::COOKIE).is_none());
        let mut h = HeaderMap::new();
        h.insert(header::CONNECTION, HeaderValue::from_static("keep-alive, x-secret"));
        h.insert("x-secret", HeaderValue::from_static("1"));
        h.insert("x-kept", HeaderValue::from_static("1"));
        drop_hop_headers(&mut h);
        assert!(h.get("x-secret").is_none() && h.get(header::CONNECTION).is_none());
        assert!(h.get("x-kept").is_some());
    }
}
