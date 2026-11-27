//! Les mises à jour de bout en bout, avec un faux GitHub et un faux programme du Core : recherche
//! (Core et applis, applis qui n'accepteraient pas le nouveau Core), mise à jour du Core (empreinte,
//! essai du nouveau programme, remplacement, ancien gardé, redémarrage demandé), archive envoyée,
//! refus.
use axum::{
    body::Body,
    extract::{connect_info::MockConnectInfo, Path as UrlPath, State},
    http::{header, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use strata_core_server::{
    auth::NewUser,
    state::{AppState, Shared},
};
use tower::ServiceExt;

/// Une archive du Core dont le programme dit `strata-core <says>`.
fn core_archive(says: &str, name: &str) -> Vec<u8> {
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
    let files =
        [(format!("strata-core-x/{name}"), format!("#!/bin/sh\necho \"strata-core {says}\"\n"), 0o755), ("strata-core-x/README.md".into(), "hi".into(), 0o644)];
    for (path, text, mode) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(text.len() as u64);
        h.set_mode(mode);
        h.set_cksum();
        tar.append_data(&mut h, path, text.as_bytes()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

fn sha(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

#[derive(Default)]
struct Gh {
    releases: Mutex<Vec<(String, Value)>>,
    assets: Mutex<Vec<Vec<u8>>>,
}

impl Gh {
    fn asset(&self, b: Vec<u8>) -> usize {
        let mut a = self.assets.lock().unwrap();
        a.push(b);
        a.len() - 1
    }

    /// Publie une release : `name` (strata-core ou l'id d'une appli), une archive et sa fiche.
    #[allow(clippy::too_many_arguments)]
    fn publish(&self, repo: &str, name: &str, version: &str, archive: Vec<u8>, archive_name: &str, kind: &str, sum: Option<&str>, requires: Value) {
        let sum = sum.map(str::to_string).unwrap_or_else(|| sha(&archive));
        let size = archive.len();
        let a = self.asset(archive);
        let file =
            json!({"name": name, "version": version, "requires": requires, "assets": [{"name": archive_name, "kind": kind, "size": size, "sha256": sum}]});
        let f = self.asset(file.to_string().into_bytes());
        let rel = json!({"tag_name": format!("v{version}"), "draft": false, "prerelease": false,
            "assets": [{"id": f, "name": "release.json", "size": 1}, {"id": a, "name": archive_name, "size": size}]});
        self.releases.lock().unwrap().insert(0, (repo.into(), rel));
    }
}

async fn fake_github() -> (Arc<Gh>, String) {
    async fn releases(State(g): State<Arc<Gh>>, UrlPath((o, r)): UrlPath<(String, String)>) -> Response {
        let repo = format!("{o}/{r}");
        let list: Vec<Value> = g.releases.lock().unwrap().iter().filter(|(rp, _)| *rp == repo).map(|(_, v)| v.clone()).collect();
        axum::Json(list).into_response()
    }
    async fn asset(State(g): State<Arc<Gh>>, UrlPath((_, _, id)): UrlPath<(String, String, usize)>) -> Response {
        g.assets.lock().unwrap().get(id).cloned().map(IntoResponse::into_response).unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
    }
    let g = Arc::new(Gh::default());
    let app = Router::new().route("/repos/{o}/{r}/releases", get(releases)).route("/repos/{o}/{r}/releases/assets/{id}", get(asset)).with_state(g.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (g, url)
}

struct Srv {
    dir: PathBuf,
    exe: PathBuf,
    s: Shared,
    app: Router,
    cookie: String,
}

impl Drop for Srv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Srv {
    async fn new() -> (Self, Arc<Gh>) {
        let dir = std::env::temp_dir().join(format!("strata-updates-{}", strata_core::new_id()));
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("config.json"), json!({"name": "Régie A", "dnsName": "strata.test", "updates": {"coreRepo": "acme/core"}}).to_string())
            .unwrap();
        // Une appli installée depuis GitHub, qui n'accepte que les Core 0.x.
        let clock = dir.join("apps/clock");
        std::fs::create_dir_all(&clock).unwrap();
        std::fs::write(
            clock.join("strata-app.toml"),
            "id = \"clock\"\nname = \"Horloge\"\nversion = \"1.0.0\"\nport = 8600\n\n[requires]\ncore = \">=0.1, <1\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("sources.json"), r#"{"apps":{"clock":{"repo":"acme/clock"}}}"#).unwrap();
        // Le « programme du Core » à remplacer.
        let exe = dir.join("bin/strata-core");
        std::fs::write(&exe, "old core").unwrap();
        let s = AppState::new(dir.clone());
        let (gh, url) = fake_github().await;
        s.installer.set_api(&url);
        s.updates.set_exe(exe.clone());
        let admin = s.auth.add(NewUser { username: "admin".into(), display_name: String::new(), admin: true, password: Some("password1".into()) }).unwrap();
        let cookie = format!("strata_session={}", s.auth.open_session(&admin.id));
        let lo: SocketAddr = "127.0.0.1:50000".parse().unwrap();
        let app = strata_core_server::app(s.clone(), None).layer(MockConnectInfo(lo));
        (Srv { dir, exe, s, app, cookie }, gh)
    }

    async fn call(&self, method: &str, path: &str, body: Option<Body>, json_body: bool) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(path).header(header::HOST, "strata.test").header(header::COOKIE, &self.cookie);
        if json_body {
            req = req.header(header::CONTENT_TYPE, "application/json");
        }
        let res = self.app.clone().oneshot(req.body(body.unwrap_or_else(Body::empty)).unwrap()).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into())))
    }

    /// Attend la fin de la mise à jour du Core.
    async fn wait(&self) -> Value {
        for _ in 0..200 {
            let (_, v) = self.call("GET", "/api/updates", None, false).await;
            if v["job"]["step"] == "done" || v["job"]["step"] == "failed" {
                return v["job"].clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!("the Core update never ended");
    }
}

#[tokio::test]
async fn check_then_update_the_core() {
    let (srv, gh) = Srv::new().await;
    let core = strata_core::VERSION;
    gh.publish(
        "acme/core",
        "strata-core",
        "9.9.0",
        core_archive("9.9.0", "strata-core"),
        "strata-core-9.9.0-linux-x86_64.tar.gz",
        "core-linux-x86_64",
        None,
        json!({}),
    );
    gh.publish("acme/clock", "clock", "1.1.0", vec![1], "clock-1.1.0-linux-x86_64.tar.gz", "app-linux-x86_64", None, json!({"core": ">=0.1"}));

    // La recherche : le Core et l'appli ont une nouvelle version ; l'appli n'accepterait pas le Core 9.
    let (st, v) = srv.call("POST", "/api/updates/check", None, false).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["core"]["version"], core);
    assert_eq!(v["checked"]["core"]["latest"], "9.9.0");
    assert_eq!(v["checked"]["core"]["newer"], true);
    assert_eq!(v["checked"]["core"]["blocked"], json!([{"id": "clock", "name": "Horloge", "version": "1.0.0", "needs": ">=0.1, <1"}]));
    assert_eq!(v["checked"]["apps"]["clock"], json!({"latest": "1.1.0", "newer": true}));
    assert_eq!(v["count"], 2);
    assert!(v["checked"]["at"].as_u64().is_some());
    assert_eq!(v["daily"], true);

    // La mise à jour : téléchargée, vérifiée, essayée, mise en place ; l'ancienne gardée ; redémarrage demandé.
    let (st, _) = srv.call("POST", "/api/updates/core", None, false).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    let job = srv.wait().await;
    assert_eq!((job["step"].as_str(), job["version"].as_str()), (Some("done"), Some("9.9.0")), "{job}");
    assert!(std::fs::read_to_string(&srv.exe).unwrap().contains("strata-core 9.9.0"));
    assert_eq!(std::fs::read_to_string(srv.dir.join("bin/strata-core.previous")).unwrap(), "old core");
    for _ in 0..100 {
        if srv.s.restart_asked() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(srv.s.restart_asked());
    let mut leftovers: Vec<String> = std::fs::read_dir(srv.dir.join("bin")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into()).collect();
    leftovers.sort();
    // Le nouveau, l'ancien, et le marqueur à confirmer (l'ancien revient s'il ne tient pas).
    assert_eq!(leftovers, ["strata-core", "strata-core.pending", "strata-core.previous"], "{leftovers:?}");
    // Trois démarrages ratés : l'ancien est remis.
    for _ in 0..3 {
        assert!(!strata_core_server::updates::guard_start(&srv.exe));
    }
    assert!(strata_core_server::updates::guard_start(&srv.exe));
    assert_eq!(std::fs::read_to_string(&srv.exe).unwrap(), "old core");
    assert!(!srv.dir.join("bin/strata-core.pending").exists());
}

#[tokio::test]
async fn refusals() {
    let (srv, gh) = Srv::new().await;
    // Le programme ne dit pas la version annoncée : rien n'est remplacé.
    gh.publish(
        "acme/core",
        "strata-core",
        "9.9.0",
        core_archive("9.8.0", "strata-core"),
        "strata-core-9.9.0-linux-x86_64.tar.gz",
        "core-linux-x86_64",
        None,
        json!({}),
    );
    srv.call("POST", "/api/updates/core", None, false).await;
    assert_eq!(srv.wait().await["error"], "the archive holds the Core 9.8.0, not 9.9.0");
    assert_eq!(std::fs::read_to_string(&srv.exe).unwrap(), "old core");
    assert!(!srv.dir.join("bin/strata-core.previous").exists());

    // Une empreinte fausse.
    gh.publish(
        "acme/core",
        "strata-core",
        "9.9.1",
        core_archive("9.9.1", "strata-core"),
        "strata-core-9.9.1-linux-x86_64.tar.gz",
        "core-linux-x86_64",
        Some(&"0".repeat(64)),
        json!({}),
    );
    srv.call("POST", "/api/updates/core", None, false).await;
    assert_eq!(srv.wait().await["error"], "the download does not match its fingerprint");

    // Des archives envoyées : un Core plus ancien, pas de Core, pas une archive.
    let older = core_archive("0.0.1", "strata-core");
    let (st, v) = srv.call("POST", "/api/updates/core/upload", Some(Body::from(older)), false).await;
    assert_eq!(
        (st, v.as_str().unwrap()),
        (StatusCode::BAD_REQUEST, format!("this archive holds the Core 0.0.1; this one is already {}", strata_core::VERSION).as_str())
    );
    let (st, v) = srv.call("POST", "/api/updates/core/upload", Some(Body::from(core_archive("9.9.9", "other"))), false).await;
    assert_eq!((st, v.as_str().unwrap()), (StatusCode::BAD_REQUEST, "the archive holds no strata-core program"));
    let (st, v) = srv.call("POST", "/api/updates/core/upload", Some(Body::from("nope")), false).await;
    assert_eq!((st, v.as_str().unwrap()), (StatusCode::BAD_REQUEST, "this is not a .tar.gz archive"));
    assert_eq!(std::fs::read_to_string(&srv.exe).unwrap(), "old core");
    assert!(!srv.s.restart_asked());

    // La bonne archive envoyée : mise en place, redémarrage demandé.
    let (st, v) = srv.call("POST", "/api/updates/core/upload", Some(Body::from(core_archive("9.9.9", "strata-core"))), false).await;
    assert_eq!((st, v["version"].as_str()), (StatusCode::OK, Some("9.9.9")), "{v}");
    assert!(std::fs::read_to_string(&srv.exe).unwrap().contains("9.9.9"));

    // La clé du dépôt du Core et la recherche quotidienne.
    let (_, v) = srv.call("PUT", "/api/updates/core/key", Some(Body::from(r#"{"key":"s3cret"}"#)), true).await;
    assert_eq!(v["core"]["key"], true);
    assert!(!v.to_string().contains("s3cret"));
    let (_, v) = srv.call("PUT", "/api/updates/settings", Some(Body::from(r#"{"daily":false}"#)), true).await;
    assert_eq!(v["daily"], false);
}
