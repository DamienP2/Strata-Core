//! Les réglages de bout en bout : Bibliothèque (applis) et Utilisateurs, réservés aux administrateurs.
use axum::{
    body::Body,
    extract::connect_info::MockConnectInfo,
    http::{header, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::{net::SocketAddr, path::PathBuf};
use strata_core_server::state::AppState;
use tower::ServiceExt;

const HOST: &str = "strata01.regie.exemple.fr";

struct Srv {
    dir: PathBuf,
    app: Router,
}

impl Drop for Srv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Srv {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("strata-settings-{}", strata_core::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), format!(r#"{{"name":"Régie A","dnsName":"{HOST}"}}"#)).unwrap();
        let lo: SocketAddr = "127.0.0.1:50000".parse().unwrap();
        let app = strata_core_server::app(AppState::new(dir.clone()), None).layer(MockConnectInfo(lo));
        Srv { dir, app }
    }

    async fn call(&self, method: &str, path: &str, cookie: Option<&str>, body: Option<Value>) -> (StatusCode, Option<String>, Value) {
        let host = if path == "/api/auth/setup" { "localhost:8080" } else { HOST };
        let mut req = Request::builder().method(method).uri(path).header(header::HOST, host);
        if let Some(c) = cookie {
            req = req.header(header::COOKIE, c);
        }
        let req = match body {
            Some(b) => req.header(header::CONTENT_TYPE, "application/json").body(Body::from(b.to_string())).unwrap(),
            None => req.body(Body::empty()).unwrap(),
        };
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let cookie = res.headers().get(header::SET_COOKIE).map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string());
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let value = serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()));
        (status, cookie, value)
    }

    async fn sign_in(&self, username: &str, password: &str) -> String {
        let (st, cookie, v) = self.call("POST", "/api/auth/login", None, Some(json!({"username": username, "password": password}))).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        cookie.unwrap()
    }

    fn install(&self, id: &str, name: &str, core: &str) {
        let dir = self.dir.join("apps").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("strata-app.toml"),
            format!("id = \"{id}\"\nname = \"{name}\"\nversion = \"1.2.0\"\nicon = \"layers\"\nport = 8102\n\n[requires]\ncore = \"{core}\"\n"),
        )
        .unwrap();
    }
}

#[tokio::test]
async fn users() {
    let srv = Srv::new();
    let (_, admin, v) = srv.call("POST", "/api/auth/setup", None, Some(json!({"username": "damien", "password": "password1"}))).await;
    let admin = admin.unwrap();
    let my_id = v["user"]["id"].as_str().unwrap().to_string();

    // Créer un compte
    let (st, _, claire) =
        srv.call("POST", "/api/users", Some(&admin), Some(json!({"username": "c.martin", "displayName": "Claire Martin", "password": "password1"}))).await;
    assert_eq!(st, StatusCode::OK, "{claire}");
    assert_eq!(claire["admin"], false);
    let claire_id = claire["id"].as_str().unwrap().to_string();
    let (st, _, v) = srv.call("POST", "/api/users", Some(&admin), Some(json!({"username": "C.Martin", "password": "password1"}))).await;
    assert_eq!((st, v.as_str().unwrap()), (StatusCode::BAD_REQUEST, "this user name is already taken"));
    let (st, _, _) = srv.call("POST", "/api/users", Some(&admin), Some(json!({"username": "hugo", "password": "short"}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (_, _, v) = srv.call("GET", "/api/users", Some(&admin), None).await;
    assert_eq!(v["users"].as_array().unwrap().len(), 2);
    assert!(v["users"][0].get("passwordHash").is_none(), "hashes never leave the server");

    // Un utilisateur n'a pas accès aux réglages
    let claire_c = srv.sign_in("c.martin", "password1").await;
    for (m, p) in [("GET", "/api/users"), ("GET", "/api/apps")] {
        let (st, _, _) = srv.call(m, p, Some(&claire_c), None).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{m} {p}");
    }

    // Se désactiver, se retirer l'administration ou se supprimer soi-même : refusé
    for body in [json!({"disabled": true}), json!({"admin": false})] {
        let (st, _, _) = srv.call("PATCH", &format!("/api/users/{my_id}"), Some(&admin), Some(body)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }
    let (st, _, _) = srv.call("DELETE", &format!("/api/users/{my_id}"), Some(&admin), None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // Modifier
    let (st, _, v) = srv.call("PATCH", &format!("/api/users/{claire_id}"), Some(&admin), Some(json!({"displayName": "Claire M.", "admin": true}))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!((v["displayName"].as_str(), v["admin"].as_bool()), (Some("Claire M."), Some(true)));

    // Nouveau mot de passe : ses sessions s'arrêtent
    let (st, _, _) = srv.call("POST", &format!("/api/users/{claire_id}/password"), Some(&admin), Some(json!({"password": "password2"}))).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _, _) = srv.call("GET", "/api/cockpit", Some(&claire_c), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    srv.sign_in("c.martin", "password2").await;

    // Désactivé : ne se connecte plus
    let (st, _, _) = srv.call("PATCH", &format!("/api/users/{claire_id}"), Some(&admin), Some(json!({"disabled": true}))).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _, _) = srv.call("POST", "/api/auth/login", None, Some(json!({"username": "c.martin", "password": "password2"}))).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Groupes : appartenance, groupe inconnu refusé, un groupe supprimé en sort ses membres
    let (_, _, g) = srv.call("POST", "/api/groups", Some(&admin), Some(json!({"name": "Montage"}))).await;
    let gid = g["id"].as_str().unwrap().to_string();
    let (st, _, v) = srv.call("PATCH", &format!("/api/users/{claire_id}"), Some(&admin), Some(json!({"groups": [gid]}))).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["groupNames"], json!(["Montage"]));
    let (st, _, _) = srv.call("PATCH", &format!("/api/users/{claire_id}"), Some(&admin), Some(json!({"groups": ["ghost"]}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (_, _, v) = srv.call("GET", "/api/groups", Some(&admin), None).await;
    assert_eq!(v["groups"][0]["members"], 1);
    let (st, _, _) = srv.call("DELETE", &format!("/api/groups/{gid}"), Some(&admin), None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (_, _, v) = srv.call("GET", "/api/users", Some(&admin), None).await;
    assert!(v["users"].as_array().unwrap().iter().all(|u| u["groups"] == json!([])));

    // Supprimer
    let (st, _, _) = srv.call("DELETE", &format!("/api/users/{claire_id}"), Some(&admin), None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _, _) = srv.call("DELETE", &format!("/api/users/{claire_id}"), Some(&admin), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn library() {
    let srv = Srv::new();
    let (_, admin, _) = srv.call("POST", "/api/auth/setup", None, Some(json!({"username": "damien", "password": "password1"}))).await;
    let admin = admin.unwrap();

    let (st, _, v) = srv.call("GET", "/api/apps", Some(&admin), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["apps"], json!([]));
    assert_eq!(v["core"], strata_core::VERSION);

    srv.install("graphics", "Graphics", ">=0.1, <1");
    srv.install("network", "Network", ">=9");
    let (_, _, v) = srv.call("GET", "/api/apps", Some(&admin), None).await;
    assert_eq!(v["apps"][0]["name"], "Graphics");
    assert_eq!(v["apps"][0]["active"], false);
    assert_eq!(v["apps"][0]["url"], format!("https://graphics.{HOST}/"));
    assert_eq!(v["apps"][1]["compatible"], false);
    assert_eq!(v["apps"][1]["requires"], ">=9");

    let (st, _, v) = srv.call("PATCH", "/api/apps/graphics", Some(&admin), Some(json!({"active": true}))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["active"], true);
    let (_, _, v) = srv.call("GET", "/api/cockpit", Some(&admin), None).await;
    assert_eq!(v["apps"][0]["id"], "graphics");

    let (st, _, v) = srv.call("PATCH", "/api/apps/network", Some(&admin), Some(json!({"active": true}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(v.as_str().unwrap().contains("needs the Core >=9"), "{v}");
    let (st, _, _) = srv.call("PATCH", "/api/apps/ghost", Some(&admin), Some(json!({"active": true}))).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, _, v) = srv.call("PATCH", "/api/apps/graphics", Some(&admin), Some(json!({"active": false}))).await;
    assert_eq!((st, v["active"].as_bool()), (StatusCode::OK, Some(false)));
    let (_, _, v) = srv.call("GET", "/api/cockpit", Some(&admin), None).await;
    assert_eq!(v["apps"], json!([]));
}
