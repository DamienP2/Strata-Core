//! L'API de bout en bout : premier compte, connexion, cookie, cockpit, préférences, CSRF.
use axum::{
    body::Body,
    extract::connect_info::MockConnectInfo,
    http::{header, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::{net::SocketAddr, path::PathBuf};
use strata_core_server::{apps::Apps, state::AppState};
use tower::ServiceExt;

struct Srv {
    dir: PathBuf,
    local: Router,
    remote: Router,
}

impl Drop for Srv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn server() -> Srv {
    let dir = std::env::temp_dir().join(format!("strata-api-{}", strata_core::new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.json"), r#"{"name":"Régie A","dnsName":"strata01.regie.exemple.fr","https":{"mode":"proxy"}}"#).unwrap();
    let s = AppState::new(dir.clone());
    let app = strata_core_server::app(s, None);
    let lo: SocketAddr = "127.0.0.1:50000".parse().unwrap();
    let far: SocketAddr = "10.0.0.50:50000".parse().unwrap();
    Srv { local: app.clone().layer(MockConnectInfo(lo)), remote: app.layer(MockConnectInfo(far)), dir }
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    host: &str,
    cookie: Option<&str>,
    body: Option<Value>,
    origin: Option<&str>,
) -> (StatusCode, Option<String>, Value) {
    let mut req = Request::builder().method(method).uri(path).header(header::HOST, host);
    if let Some(c) = cookie {
        req = req.header(header::COOKIE, c);
    }
    if let Some(o) = origin {
        req = req.header(header::ORIGIN, o);
    }
    let req = match body {
        Some(b) => req.header(header::CONTENT_TYPE, "application/json").body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let set_cookie = res.headers().get(header::SET_COOKIE).map(|v| v.to_str().unwrap().to_string());
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()));
    (status, set_cookie, value)
}

fn session(set_cookie: &str) -> String {
    set_cookie.split(';').next().unwrap().to_string()
}

#[tokio::test]
async fn first_account_sign_in_and_cockpit() {
    let srv = server();
    let host = "strata01.regie.exemple.fr";

    let (st, _, v) = call(&srv.local, "GET", "/api/health", "localhost:8080", None, None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["needsSetup"], true);
    assert_eq!(v["name"], "Régie A");

    // Le premier compte : depuis le serveur lui-même seulement.
    let admin = json!({"username": "damien", "password": "password1"});
    let (st, _, _) = call(&srv.remote, "POST", "/api/auth/setup", host, None, Some(admin.clone()), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, cookie, v) = call(&srv.local, "POST", "/api/auth/setup", "localhost:8080", None, Some(admin.clone()), None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["user"]["admin"], true);
    assert!(!cookie.unwrap().contains("Domain="), "no domain on localhost");
    let (st, _, _) = call(&srv.local, "POST", "/api/auth/setup", "localhost:8080", None, Some(admin), None).await;
    assert_eq!(st, StatusCode::CONFLICT);

    // Connexion par le nom du serveur : le cookie vaut pour ses sous-domaines (les applis).
    let (st, _, v) = call(&srv.remote, "POST", "/api/auth/login", host, None, Some(json!({"username": "damien", "password": "nope"})), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(v, "wrong user name or password");
    let (st, cookie, _) = call(&srv.remote, "POST", "/api/auth/login", host, None, Some(json!({"username": "Damien", "password": "password1"})), None).await;
    assert_eq!(st, StatusCode::OK);
    let cookie = cookie.unwrap();
    assert!(cookie.contains("; Domain=strata01.regie.exemple.fr"), "{cookie}");
    let c = session(&cookie);

    // Le cockpit : connecté seulement ; vide, puis avec une appli active.
    let (st, _, _) = call(&srv.remote, "GET", "/api/cockpit", host, None, None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _, v) = call(&srv.remote, "GET", "/api/cockpit", host, Some(&c), None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["apps"], json!([]));
    assert_eq!(v["admin"], true);
    assert_eq!(v["servers"][0]["name"], "Régie A");

    let app_dir = srv.dir.join("apps/livecut");
    std::fs::create_dir_all(&app_dir).unwrap();
    std::fs::write(
        app_dir.join("strata-app.toml"),
        "id = \"livecut\"\nname = \"LiveCut\"\nversion = \"0.3.0\"\nicon = \"scissors\"\nport = 8101\n\n[requires]\ncore = \">=0.1, <1\"\n",
    )
    .unwrap();
    let (_, _, v) = call(&srv.remote, "GET", "/api/cockpit", host, Some(&c), None, None).await;
    assert_eq!(v["apps"], json!([]), "installed but inactive");
    Apps::new(&srv.dir).set_active("livecut", true).unwrap();
    let (_, _, v) = call(&srv.remote, "GET", "/api/cockpit", host, Some(&c), None, None).await;
    assert_eq!(v["apps"][0]["name"], "LiveCut");
    assert_eq!(v["apps"][0]["icon"], "scissors");
    assert_eq!(v["instances"][0]["url"], "https://livecut.strata01.regie.exemple.fr/");
    assert_eq!(v["instances"][0]["server"], v["servers"][0]["id"]);
}

#[tokio::test]
async fn prefs_csrf_and_logout() {
    let srv = server();
    let host = "strata01.regie.exemple.fr";
    let (_, cookie, _) =
        call(&srv.local, "POST", "/api/auth/setup", "localhost:8080", None, Some(json!({"username": "admin", "password": "password1"})), None).await;
    let c = session(&cookie.unwrap());

    // Même site : accepté.
    let (st, _, v) =
        call(&srv.remote, "PUT", "/api/me/prefs", host, Some(&c), Some(json!({"lang": "fr", "theme": "light"})), Some(&format!("https://{host}"))).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["lang"], "fr");
    assert_eq!(v["theme"], "light");
    let (_, _, v) = call(&srv.remote, "GET", "/api/auth/state", host, Some(&c), None, None).await;
    assert_eq!(v["user"]["theme"], "light");

    // Un autre site avec le seul cookie : refusé.
    let (st, _, v) = call(&srv.remote, "PUT", "/api/me/prefs", host, Some(&c), Some(json!({"lang": "en"})), Some("https://evil.example.com")).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(v, "request sent by another site refused");
    let (st, _, _) = call(&srv.remote, "PUT", "/api/me/prefs", host, Some(&c), Some(json!({"theme": "pink"})), None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // Déconnexion : la session ne vaut plus.
    let (st, cookie, _) = call(&srv.remote, "POST", "/api/auth/logout", host, Some(&c), None, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(cookie.unwrap().contains("Max-Age=0"));
    let (st, _, _) = call(&srv.remote, "GET", "/api/cockpit", host, Some(&c), None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // L'interface : une page qui n'est pas un fichier renvoie l'interface (ou le message « pas construite »).
    let (st, _, _) = call(&srv.remote, "GET", "/reglages", host, None, None, None).await;
    assert!(st == StatusCode::OK || st == StatusCode::SERVICE_UNAVAILABLE);
    let (st, _, _) = call(&srv.remote, "GET", "/api/nothing", host, None, None, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}
