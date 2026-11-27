//! Installer depuis la Bibliothèque, de bout en bout, avec un faux GitHub : choix de la release
//! (préversions et versions qui n'acceptent pas ce Core écartées), dépôt privé et sa clé, empreinte,
//! mise à jour, archive envoyée, désinstallation.
use axum::{
    body::Body,
    extract::{connect_info::MockConnectInfo, Path as UrlPath, State},
    http::{header, HeaderMap, Request, StatusCode},
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
use strata_core_server::{auth::NewUser, state::AppState};
use tower::ServiceExt;

const HOST: &str = "strata.test";
const KEY: &str = "s3cret";

/// Une archive d'appli : sa fiche, un programme, un fichier ; dans un dossier ou à la racine.
fn archive(id: &str, version: &str, core: &str, port: u16, in_dir: bool) -> Vec<u8> {
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
    let base = if in_dir { format!("{id}-{version}/") } else { String::new() };
    let files: [(&str, String, u32); 3] = [
        ("strata-app.toml", format!("id = \"{id}\"\nname = \"App {id}\"\nversion = \"{version}\"\nport = {port}\n\n[requires]\ncore = \"{core}\"\n"), 0o644),
        ("bin/run.sh", "#!/bin/sh\nexec sleep 30\n".into(), 0o755),
        ("share/version.txt", version.into(), 0o644),
    ];
    for (name, text, mode) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(text.len() as u64);
        h.set_mode(mode);
        h.set_cksum();
        tar.append_data(&mut h, format!("{base}{name}"), text.as_bytes()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

fn sha(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// Le faux GitHub : des releases par dépôt, leurs fichiers, et les clés reçues.
#[derive(Default)]
struct Gh {
    releases: Mutex<Vec<(String, Value)>>,
    assets: Mutex<Vec<Vec<u8>>>,
    keys: Mutex<Vec<String>>,
}

impl Gh {
    fn asset(&self, bytes: Vec<u8>) -> usize {
        let mut a = self.assets.lock().unwrap();
        a.push(bytes);
        a.len() - 1
    }

    /// Publie une release : l'archive, et sa fiche (avec l'empreinte donnée, ou la vraie).
    fn publish(&self, repo: &str, id: &str, version: &str, core: &str, prerelease: bool, bad_sum: bool) {
        let tgz = archive(id, version, core, 8400, true);
        let name = format!("{id}-{version}-linux-x86_64.tar.gz");
        let sum = if bad_sum { "0".repeat(64) } else { sha(&tgz) };
        let size = tgz.len();
        let tgz_id = self.asset(tgz);
        let file = json!({"name": id, "version": version, "tag": format!("v{version}"), "requires": {"core": core},
            "assets": [{"name": name, "kind": "app-linux-x86_64", "size": size, "sha256": sum}]});
        let rel_id = self.asset(file.to_string().into_bytes());
        let release = json!({"tag_name": format!("v{version}"), "draft": false, "prerelease": prerelease, "assets": [
            {"id": rel_id, "name": "release.json", "size": 10},
            {"id": tgz_id, "name": name, "size": size},
        ]});
        // Les plus récentes d'abord, comme GitHub.
        self.releases.lock().unwrap().insert(0, (repo.to_string(), release));
    }

    fn allowed(&self, repo: &str, h: &HeaderMap) -> bool {
        let auth = h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        if !auth.is_empty() {
            self.keys.lock().unwrap().push(auth.clone());
        }
        !repo.starts_with("acme/private") || auth == format!("Bearer {KEY}")
    }
}

async fn fake_github() -> (Arc<Gh>, String) {
    async fn releases(State(g): State<Arc<Gh>>, UrlPath((o, r)): UrlPath<(String, String)>, h: HeaderMap) -> Response {
        let repo = format!("{o}/{r}");
        if !g.allowed(&repo, &h) {
            return StatusCode::NOT_FOUND.into_response();
        }
        let list: Vec<Value> = g.releases.lock().unwrap().iter().filter(|(rp, _)| *rp == repo).map(|(_, v)| v.clone()).collect();
        if list.is_empty() && !repo.starts_with("acme/") {
            return StatusCode::NOT_FOUND.into_response();
        }
        axum::Json(list).into_response()
    }
    async fn asset(State(g): State<Arc<Gh>>, UrlPath((o, r, id)): UrlPath<(String, String, usize)>, h: HeaderMap) -> Response {
        if !g.allowed(&format!("{o}/{r}"), &h) {
            return StatusCode::NOT_FOUND.into_response();
        }
        match g.assets.lock().unwrap().get(id) {
            Some(b) => b.clone().into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
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
        let dir = std::env::temp_dir().join(format!("strata-install-{}", strata_core::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), format!(r#"{{"name":"Régie A","dnsName":"{HOST}"}}"#)).unwrap();
        let s = AppState::new(dir.clone());
        let (gh, url) = fake_github().await;
        s.installer.set_api(&url);
        let admin = s.auth.add(NewUser { username: "admin".into(), display_name: String::new(), admin: true, password: Some("password1".into()) }).unwrap();
        let cookie = format!("strata_session={}", s.auth.open_session(&admin.id));
        let lo: SocketAddr = "127.0.0.1:50000".parse().unwrap();
        let app = strata_core_server::app(s, None).layer(MockConnectInfo(lo));
        (Srv { dir, app, cookie }, gh)
    }

    async fn call(&self, method: &str, path: &str, body: Option<Body>, json_body: bool) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(path).header(header::HOST, HOST).header(header::COOKIE, &self.cookie);
        if json_body {
            req = req.header(header::CONTENT_TYPE, "application/json");
        }
        let res = self.app.clone().oneshot(req.body(body.unwrap_or_else(Body::empty)).unwrap()).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into())))
    }

    async fn post(&self, path: &str, v: Value) -> (StatusCode, Value) {
        self.call("POST", path, Some(Body::from(v.to_string())), true).await
    }

    async fn apps(&self) -> Value {
        self.call("GET", "/api/apps", None, false).await.1
    }

    /// Attend la fin de l'installation depuis `repo` ; rend son état final.
    async fn wait(&self, repo: &str) -> Value {
        for _ in 0..200 {
            let v = self.apps().await;
            if let Some(j) = v["jobs"].as_array().unwrap().iter().find(|j| j["repo"] == repo) {
                if j["step"] == "done" || j["step"] == "failed" {
                    return j.clone();
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!("the install from {repo} never ended");
    }
}

#[tokio::test]
async fn from_github() {
    let (srv, gh) = Srv::new().await;
    gh.publish("acme/clock", "clock", "1.0.0", ">=0.1", false, false);
    gh.publish("acme/clock", "clock", "2.0.0", ">=0.1", false, false);
    gh.publish("acme/clock", "clock", "2.1.0-rc.1", ">=0.1", true, false);
    gh.publish("acme/clock", "clock", "3.0.0", ">=99", false, false);

    // La plus récente qui accepte ce Core, hors préversion : 2.0.0. L'adresse du dépôt est acceptée.
    let (st, job) = srv.post("/api/apps/install", json!({"repo": "https://github.com/acme/clock"})).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{job}");
    assert_eq!(job["repo"], "acme/clock");
    let (st, again) = srv.post("/api/apps/install", json!({"repo": "acme/clock"})).await;
    assert!(st == StatusCode::BAD_REQUEST && again.as_str().unwrap().contains("already being installed") || st == StatusCode::ACCEPTED);
    let end = srv.wait("acme/clock").await;
    assert_eq!(end["step"], "done", "{end}");
    assert_eq!(end["version"], "2.0.0");
    let apps = srv.apps().await;
    let clock = &apps["apps"][0];
    assert_eq!((clock["id"].as_str(), clock["version"].as_str(), clock["active"].as_bool()), (Some("clock"), Some("2.0.0"), Some(false)));
    assert_eq!(clock["source"], json!({"repo": "acme/clock", "key": false}));
    let dir = srv.dir.join("apps/clock");
    assert_eq!(std::fs::read_to_string(dir.join("share/version.txt")).unwrap(), "2.0.0", "the archive's own folder is unwrapped");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(dir.join("bin/run.sh")).unwrap().permissions().mode() & 0o111, 0o111, "programs stay executable");
    }
    assert!(std::fs::read_dir(srv.dir.join("apps")).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with('.')), "nothing left behind");

    // Rien de plus récent ; puis une 2.2.0 : mise à jour, sans toucher à ses données.
    let (_, up) = srv.call("GET", "/api/apps/clock/update", None, false).await;
    assert_eq!(up, json!({"current": "2.0.0", "latest": "2.0.0", "newer": false}));
    std::fs::create_dir_all(srv.dir.join("app-data/clock")).unwrap();
    std::fs::write(srv.dir.join("app-data/clock/state.json"), "{}").unwrap();
    gh.publish("acme/clock", "clock", "2.2.0", ">=0.1", false, false);
    let (_, up) = srv.call("GET", "/api/apps/clock/update", None, false).await;
    assert_eq!(up["newer"], true);
    let (st, _) = srv.call("POST", "/api/apps/clock/update", None, false).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    assert_eq!(srv.wait("acme/clock").await["version"], "2.2.0");
    assert_eq!(std::fs::read_to_string(dir.join("share/version.txt")).unwrap(), "2.2.0");
    assert!(srv.dir.join("app-data/clock/state.json").exists());

    // Désinstaller : refusé tant qu'elle est active ; ses données gardées, sauf demande.
    srv.call("PATCH", "/api/apps/clock", Some(Body::from(r#"{"active":true}"#)), true).await;
    let (st, msg) = srv.call("DELETE", "/api/apps/clock", None, false).await;
    assert_eq!((st, msg.as_str().unwrap()), (StatusCode::BAD_REQUEST, "App clock is active: deactivate it first"));
    srv.call("PATCH", "/api/apps/clock", Some(Body::from(r#"{"active":false}"#)), true).await;
    let (st, _) = srv.call("DELETE", "/api/apps/clock", None, false).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(!dir.exists());
    assert!(srv.dir.join("app-data/clock/state.json").exists(), "data kept");
    assert_eq!(srv.apps().await["apps"], json!([]));
    let (st, _) = srv.call("DELETE", "/api/apps/clock?data=true", None, false).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn refusals_and_private_repositories() {
    let (srv, gh) = Srv::new().await;

    // Rien qui accepte ce Core ; pas de release Strata ; dépôt inconnu ; empreinte fausse.
    gh.publish("acme/future", "future", "5.0.0", ">=99", false, false);
    srv.post("/api/apps/install", json!({"repo": "acme/future"})).await;
    let e = srv.wait("acme/future").await;
    assert!(e["error"].as_str().unwrap().starts_with("no release of acme/future accepts the Core"), "{e}");
    srv.post("/api/apps/install", json!({"repo": "acme/empty"})).await;
    assert_eq!(srv.wait("acme/empty").await["error"], "acme/empty has no Strata release");
    srv.post("/api/apps/install", json!({"repo": "nobody/nothing"})).await;
    assert_eq!(srv.wait("nobody/nothing").await["error"], "nobody/nothing was not found on GitHub (a private repository needs a key)");
    gh.publish("acme/tampered", "tampered", "1.0.0", ">=0.1", false, true);
    srv.post("/api/apps/install", json!({"repo": "acme/tampered"})).await;
    assert_eq!(srv.wait("acme/tampered").await["error"], "the download does not match its fingerprint");
    let (st, _) = srv.post("/api/apps/install", json!({"repo": "not a repo"})).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(srv.apps().await["apps"].as_array().unwrap().is_empty());
    srv.post("/api/apps/jobs/dismiss", json!({"repo": "acme/future"})).await;
    assert!(srv.apps().await["jobs"].as_array().unwrap().iter().all(|j| j["repo"] != "acme/future"));

    // Un dépôt privé : sans clé, introuvable ; avec, installé, et la clé gardée pour les mises à jour
    // sans jamais revenir au navigateur.
    gh.publish("acme/private-app", "secret", "1.0.0", ">=0.1", false, false);
    srv.post("/api/apps/install", json!({"repo": "acme/private-app"})).await;
    assert!(srv.wait("acme/private-app").await["error"].as_str().unwrap().contains("needs a key"));
    srv.post("/api/apps/install", json!({"repo": "acme/private-app", "key": KEY})).await;
    assert_eq!(srv.wait("acme/private-app").await["step"], "done");
    let apps = srv.apps().await;
    assert_eq!(apps["apps"][0]["source"], json!({"repo": "acme/private-app", "key": true}));
    assert!(!apps.to_string().contains(KEY));
    let (_, up) = srv.call("GET", "/api/apps/secret/update", None, false).await;
    assert_eq!(up["latest"], "1.0.0", "the kept key is used: {up}");
    assert!(gh.keys.lock().unwrap().iter().all(|k| k == &format!("Bearer {KEY}")));
    let (st, _) = srv.call("PUT", "/api/apps/secret/key", Some(Body::from(r#"{"key":""}"#)), true).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = srv.call("GET", "/api/apps/secret/update", None, false).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "no key any more");

    // Une autre appli sur le même port : refusée.
    gh.publish("acme/twin", "twin", "1.0.0", ">=0.1", false, false);
    srv.post("/api/apps/install", json!({"repo": "acme/twin"})).await;
    assert_eq!(srv.wait("acme/twin").await["error"], "App secret already uses port 8400");
}

#[tokio::test]
async fn from_an_uploaded_archive() {
    let (srv, _) = Srv::new().await;
    let (st, v) = srv.call("POST", "/api/apps/upload", Some(Body::from(archive("offline", "1.4.0", ">=0.1", 8500, false))), false).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!((v["id"].as_str(), v["version"].as_str(), v["source"].is_null()), (Some("offline"), Some("1.4.0"), true));
    assert!(srv.dir.join("apps/offline/bin/run.sh").exists());

    let (st, v) = srv.call("POST", "/api/apps/upload", Some(Body::from(archive("old", "1.0.0", ">=99", 8501, false))), false).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(v.as_str().unwrap().starts_with("App old 1.0.0 needs the Core >=99"), "{v}");
    let (st, v) = srv.call("POST", "/api/apps/upload", Some(Body::from("not an archive")), false).await;
    assert_eq!((st, v.as_str().unwrap()), (StatusCode::BAD_REQUEST, "this is not a .tar.gz archive"));

    // Une archive qui voudrait écrire hors de son dossier.
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
    let mut h = tar::Header::new_gnu();
    h.set_size(2);
    h.set_mode(0o644);
    let path = b"../../escape.txt";
    h.as_gnu_mut().unwrap().name[..path.len()].copy_from_slice(path);
    h.set_cksum();
    tar.append(&h, &b"hi"[..]).unwrap();
    let evil = tar.into_inner().unwrap().finish().unwrap();
    let (st, _) = srv.call("POST", "/api/apps/upload", Some(Body::from(evil)), false).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(!srv.dir.join("escape.txt").exists() && !srv.dir.parent().unwrap().join("escape.txt").exists());
    let names: Vec<String> = std::fs::read_dir(srv.dir.join("apps")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into()).collect();
    assert_eq!(names, ["offline"], "nothing left behind");
}
