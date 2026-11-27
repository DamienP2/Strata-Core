//! Le relais de bout en bout, sur de vraies connexions : une appli simulée derrière le Core,
//! `demo.strata.test` → son port. Connexion exigée, identité transmise, en-têtes du navigateur
//! retirés, chemins publics, protocole changé (WebSocket), appli éteinte ou inactive, cockpit.
use axum::{
    body::Body,
    extract::Request,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get},
    Json, Router,
};
use http_body_util::BodyExt;
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::{TokioExecutor, TokioIo},
};
use serde_json::{json, Map, Value};
use std::{net::SocketAddr, path::PathBuf};
use strata_core_server::{apps::Apps, auth::NewUser, state::AppState};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

/// L'appli simulée : `/echo` rend ce qu'elle a reçu ; `/raw` passe à un protocole d'écho.
async fn fake_app() -> u16 {
    async fn echo(req: Request) -> Json<Value> {
        let headers: Map<String, Value> = req.headers().iter().map(|(k, v)| (k.to_string(), json!(v.to_str().unwrap_or("")))).collect();
        Json(json!({"method": req.method().as_str(), "uri": req.uri().to_string(), "headers": headers}))
    }
    async fn raw(mut req: Request) -> Response {
        let on = hyper::upgrade::on(&mut req);
        tokio::spawn(async move {
            let Ok(up) = on.await else { return };
            let mut io = TokioIo::new(up);
            let mut buf = [0u8; 64];
            while let Ok(n) = io.read(&mut buf).await {
                if n == 0 || io.write_all(&buf[..n]).await.is_err() {
                    break;
                }
            }
        });
        Response::builder()
            .status(StatusCode::SWITCHING_PROTOCOLS)
            .header(header::CONNECTION, "upgrade")
            .header(header::UPGRADE, "echo")
            .body(Body::empty())
            .unwrap()
    }
    let app = Router::new()
        .route("/echo", any(echo))
        .route("/output/pgm", get(|| async { ([("content-security-policy", "default-src 'none'")], "program output").into_response() }))
        .route("/raw", get(raw));
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    port
}

fn install(dir: &std::path::Path, id: &str, name: &str, port: u16, extra: &str) {
    let d = dir.join("apps").join(id);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("strata-app.toml"),
        format!("id = \"{id}\"\nname = \"{name}\"\nversion = \"1.0.0\"\nport = {port}\n\n[requires]\ncore = \">=0.1\"\n{extra}"),
    )
    .unwrap();
}

struct Env {
    dir: PathBuf,
    core: u16,
    token: String,
    client: Client<HttpConnector, Body>,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Env {
    async fn get(&self, host: &str, path: &str, headers: &[(&str, &str)]) -> (StatusCode, axum::http::HeaderMap, String) {
        self.send("GET", host, path, headers).await
    }

    async fn send(&self, method: &str, host: &str, path: &str, headers: &[(&str, &str)]) -> (StatusCode, axum::http::HeaderMap, String) {
        let mut b =
            Request::builder().method(method).uri(format!("http://127.0.0.1:{}{path}", self.core)).header(header::HOST, format!("{host}:{}", self.core));
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let res = self.client.request(b.body(Body::empty()).unwrap()).await.unwrap();
        let (parts, body) = res.into_parts();
        let text = String::from_utf8_lossy(&body.collect().await.unwrap().to_bytes()).to_string();
        (parts.status, parts.headers, text)
    }

    fn cookie(&self) -> String {
        format!("strata_session={}; app_sid=42", self.token)
    }
}

async fn env() -> (Env, u16) {
    let dir = std::env::temp_dir().join(format!("strata-relay-{}", strata_core::new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.json"), r#"{"name":"Régie A","dnsName":"strata.test","scheme":"http"}"#).unwrap();
    let app_port = fake_app().await;
    install(&dir, "demo", "Démo", app_port, "\n[relay]\npublic = [\"/output/\"]\n");
    let free = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port();
    install(&dir, "off", "Éteinte", free, "");
    install(&dir, "sleepy", "Inactive", free + 1, "");
    let apps = Apps::new(&dir);
    apps.set_active("demo", true).unwrap();
    apps.set_active("off", true).unwrap();

    let s = AppState::new(dir.clone());
    let g = s.groups.add("Réalisation", vec![]).unwrap();
    s.auth.add(NewUser { username: "admin".into(), display_name: String::new(), admin: true, password: Some("password1".into()) }).unwrap();
    let u = s.auth.add(NewUser { username: "ann".into(), display_name: "Ann Lévy".into(), admin: false, password: Some("password1".into()) }).unwrap();
    s.auth
        .modify(&u.id, |u| {
            u.groups = vec![g.id.clone()];
            u.lang = "fr".into();
            Ok(())
        })
        .unwrap();
    let token = s.auth.open_session(&u.id);
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let core = l.local_addr().unwrap().port();
    s.set_core_port(core);
    let router = strata_core_server::app(s, None);
    tokio::spawn(async move { axum::serve(l, router.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap() });
    let client = Client::builder(TokioExecutor::new()).build_http();
    (Env { dir, core, token, client }, app_port)
}

const PAGE: (&str, &str) = ("accept", "text/html,application/xhtml+xml");

#[tokio::test]
async fn relays_to_the_app() {
    let (e, _) = env().await;
    let demo = "demo.strata.test";

    // Sans session : une page renvoie à la connexion du cockpit, qui ramènera ici ; le reste : 401.
    let (st, h, _) = e.get(demo, "/echo?x=1", &[PAGE]).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let to = h.get(header::LOCATION).unwrap().to_str().unwrap();
    assert_eq!(to, format!("http://strata.test:{0}/?next=http%3A%2F%2Fdemo.strata.test%3A{0}%2Fecho%3Fx%3D1", e.core));
    let (st, _, _) = e.get(demo, "/echo", &[]).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Connecté : l'appli reçoit la requête, qui est connecté, sans la session du Core ni les en-têtes
    // d'identité envoyés par le navigateur.
    let cookie = e.cookie();
    let (st, h, body) = e.get(demo, "/echo?x=1", &[("cookie", &cookie), ("x-strata-admin", "1"), ("x-strata-user", "root")]).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(h.get("content-security-policy").is_none(), "the Core's page policy is not put on the app's answers");
    let v: Value = serde_json::from_str(&body).unwrap();
    let got = &v["headers"];
    assert_eq!(v["uri"], "/echo?x=1");
    assert_eq!(got["x-strata-user"], "ann");
    assert_eq!(got["x-strata-user-name"], "Ann%20L%C3%A9vy");
    assert_eq!(got["x-strata-admin"], "0", "the browser's own header is dropped");
    assert_eq!(got["x-strata-group-names"], "R%C3%A9alisation");
    assert_eq!(got["x-strata-lang"], "fr");
    assert_eq!(got["cookie"], "app_sid=42", "the app's cookies, not the Core's session");
    assert_eq!(got["host"], format!("demo.strata.test:{}", e.core));
    assert_eq!(got["x-forwarded-for"], "127.0.0.1");
    assert_eq!(got["x-forwarded-proto"], "http");

    // Le jeton du Core en `Authorization` vaut aussi, et n'est pas transmis.
    let bearer = format!("Bearer {}", e.token);
    let (st, _, body) = e.get(demo, "/echo", &[("authorization", &bearer)]).await;
    assert_eq!(st, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert!(v["headers"].get("authorization").is_none());
    assert_eq!(v["headers"]["x-strata-user"], "ann");

    // Un changement envoyé par un autre site avec le cookie : refusé ; par l'appli elle-même : relayé.
    let (st, _, _) = e.send("POST", demo, "/echo", &[("cookie", &cookie), ("origin", "https://evil.example")]).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let origin = format!("http://demo.strata.test:{}", e.core);
    let (st, _, body) = e.send("POST", demo, "/echo", &[("cookie", &cookie), ("origin", &origin)]).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["method"], "POST");

    // Un chemin public : sans session, et sans identité même avec.
    let (st, h, body) = e.get(demo, "/output/pgm", &[]).await;
    assert_eq!((st, body.as_str()), (StatusCode::OK, "program output"));
    assert_eq!(h.get("content-security-policy").unwrap(), "default-src 'none'", "the app's own headers");
    let (st, _, _) = e.get(demo, "/echo", &[]).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "only the public paths");
}

#[tokio::test]
async fn upgrades_go_through() {
    let (e, _) = env().await;
    let ask = |origin: &str| {
        format!(
            "GET /raw HTTP/1.1\r\nHost: demo.strata.test:{0}\r\nConnection: Upgrade\r\nUpgrade: echo\r\nCookie: strata_session={1}\r\nOrigin: {origin}\r\n\r\n",
            e.core, e.token
        )
    };
    let mut c = TcpStream::connect(("127.0.0.1", e.core)).await.unwrap();
    c.write_all(ask(&format!("http://demo.strata.test:{}", e.core)).as_bytes()).await.unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        c.read_exact(&mut b).await.unwrap();
        head.push(b[0]);
    }
    let head = String::from_utf8_lossy(&head).to_lowercase();
    assert!(head.starts_with("http/1.1 101"), "{head}");
    assert!(head.contains("upgrade: echo"));
    c.write_all(b"ping").await.unwrap();
    let mut back = [0u8; 4];
    c.read_exact(&mut back).await.unwrap();
    assert_eq!(&back, b"ping");

    // Un autre site ne peut pas ouvrir de WebSocket avec le cookie.
    let mut c = TcpStream::connect(("127.0.0.1", e.core)).await.unwrap();
    c.write_all(ask("https://evil.example").as_bytes()).await.unwrap();
    let mut first = [0u8; 12];
    c.read_exact(&mut first).await.unwrap();
    assert_eq!(&first, b"HTTP/1.1 403");
}

#[tokio::test]
async fn unavailable_apps_and_cockpit() {
    let (e, _) = env().await;
    let cookie = e.cookie();
    let fr = ("accept-language", "en");

    // Active mais éteinte : une page qui se recharge, en français (la langue du compte).
    let (st, h, body) = e.get("off.strata.test", "/", &[PAGE, fr, ("cookie", &cookie)]).await;
    assert_eq!(st, StatusCode::BAD_GATEWAY);
    assert!(h.get(header::CONTENT_TYPE).unwrap().to_str().unwrap().starts_with("text/html"));
    assert!(body.contains("Éteinte ne répond pas.") && body.contains(r#"http-equiv="refresh""#), "{body}");
    assert!(body.contains(&format!("href=\"http://strata.test:{}/\"", e.core)), "back to the cockpit");
    let (st, _, body) = e.get("off.strata.test", "/api/state", &[("cookie", &cookie)]).await;
    assert_eq!((st, body.as_str()), (StatusCode::BAD_GATEWAY, "Éteinte ne répond pas."));

    // Installée mais inactive, ou inconnue : pas disponible (avant même la connexion).
    for host in ["sleepy.strata.test", "ghost.strata.test"] {
        let (st, _, body) = e.get(host, "/", &[PAGE, fr]).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert!(body.contains("This app is not available on this server."), "{body}");
    }

    // Le nom du serveur lui-même reste le cockpit ; une instance dit si son appli répond.
    let (st, _, body) = e.get("strata.test", "/api/cockpit", &[("cookie", &cookie)]).await;
    assert_eq!(st, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    let inst = v["instances"].as_array().unwrap();
    let demo = inst.iter().find(|i| i["app"] == "demo").unwrap();
    assert_eq!(demo["up"], true);
    assert_eq!(demo["url"], format!("http://demo.strata.test:{}/", e.core));
    assert_eq!(inst.iter().find(|i| i["app"] == "off").unwrap()["up"], false);
}
