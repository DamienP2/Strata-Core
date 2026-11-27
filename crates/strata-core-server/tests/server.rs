//! Ce serveur, de bout en bout sur de vraies connexions : le HTTPS fait par Strata (l'autorité à
//! télécharger, un vrai client TLS qui ne fait confiance qu'à elle), le renvoi du HTTP vers le HTTPS,
//! le changement de nom (nouveau certificat sans redémarrer), le retour au HTTP, le redémarrage.
use axum::{body::Bytes, http::Request};
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use rustls::pki_types::{pem::PemObject, CertificateDer, ServerName};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};
use strata_core_server::{auth::NewUser, state::AppState, Ending};
use tokio::net::{TcpListener, TcpStream};

struct Srv {
    dir: PathBuf,
    http: u16,
    https: u16,
    cookie: String,
    done: Option<tokio::task::JoinHandle<anyhow::Result<Ending>>>,
}

impl Drop for Srv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port()
}

async fn start() -> Srv {
    let dir = std::env::temp_dir().join(format!("strata-server-{}", strata_core::new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let https = free_port().await;
    std::fs::write(dir.join("config.json"), json!({"name": "Régie A", "dnsName": "strata.test", "https": {"mode": "strata", "port": https}}).to_string())
        .unwrap();
    let s = AppState::new(dir.clone());
    let admin = s.auth.add(NewUser { username: "admin".into(), display_name: String::new(), admin: true, password: Some("password1".into()) }).unwrap();
    let cookie = format!("strata_session={}", s.auth.open_session(&admin.id));
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = l.local_addr().unwrap().port();
    let done = tokio::spawn(strata_core_server::serve_on(s, l, None));
    // Le HTTPS démarre avec le service.
    for _ in 0..100 {
        if TcpStream::connect(("127.0.0.1", https)).await.is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    Srv { dir, http, https, cookie, done: Some(done) }
}

/// Une requête sur une connexion (déjà chiffrée ou non) ; rend le statut, les en-têtes et le corps.
async fn send<I>(io: I, method: &str, host: &str, path: &str, headers: &[(&str, &str)], body: Option<Value>) -> (u16, axum::http::HeaderMap, String)
where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut tx, conn) = hyper::client::conn::http1::handshake(TokioIo::new(io)).await.unwrap();
    tokio::spawn(conn);
    let mut b = Request::builder().method(method).uri(path).header("host", host);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let data = body.map(|v| {
        b = std::mem::take(&mut b).header("content-type", "application/json");
        v.to_string()
    });
    let res = tx.send_request(b.body(Full::new(Bytes::from(data.unwrap_or_default()))).unwrap()).await.unwrap();
    let (parts, body) = res.into_parts();
    (parts.status.as_u16(), parts.headers, String::from_utf8_lossy(&body.collect().await.unwrap().to_bytes()).into())
}

async fn plain(port: u16, method: &str, host: &str, path: &str, headers: &[(&str, &str)]) -> (u16, axum::http::HeaderMap, String) {
    send(TcpStream::connect(("127.0.0.1", port)).await.unwrap(), method, host, path, headers, None).await
}

/// Une connexion TLS qui ne fait confiance qu'à `ca`, au nom `name`.
async fn tls(port: u16, ca: &str, name: &str) -> Result<tokio_rustls::client::TlsStream<TcpStream>, std::io::Error> {
    let mut roots = rustls::RootCertStore::empty();
    for c in CertificateDer::pem_slice_iter(ca.as_bytes()) {
        roots.add(c.unwrap()).unwrap();
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let tcp = TcpStream::connect(("127.0.0.1", port)).await?;
    tokio_rustls::TlsConnector::from(Arc::new(config)).connect(ServerName::try_from(name.to_string()).unwrap(), tcp).await
}

#[tokio::test]
async fn https_made_by_strata() {
    let srv = start().await;
    let host = format!("strata.test:{}", srv.http);
    let tls_host = format!("strata.test:{}", srv.https);

    // L'autorité se télécharge en HTTP (pour l'installer avant de passer en HTTPS) ; le reste est
    // renvoyé vers le HTTPS, sauf depuis la machine elle-même.
    let (st, h, ca) = plain(srv.http, "GET", &host, "/api/https/ca.crt", &[]).await;
    assert_eq!(st, 200);
    assert!(ca.starts_with("-----BEGIN CERTIFICATE-----"));
    assert!(h.get("content-disposition").unwrap().to_str().unwrap().contains("-ca.crt"));
    let (st, h, _) = plain(srv.http, "GET", &host, "/reglages?x=1", &[]).await;
    assert_eq!(st, 308);
    assert_eq!(h.get("location").unwrap(), &format!("https://strata.test:{}/reglages?x=1", srv.https));
    let (st, h, _) = plain(srv.http, "GET", &format!("demo.strata.test:{}", srv.http), "/", &[]).await;
    assert_eq!((st, h.get("location").unwrap().to_str().unwrap()), (308, format!("https://demo.strata.test:{}/", srv.https).as_str()));
    let (st, _, _) = plain(srv.http, "POST", &host, "/api/auth/logout", &[]).await;
    assert_eq!(st, 421, "a change is not replayed elsewhere");
    let (st, _, _) = plain(srv.http, "GET", &format!("localhost:{}", srv.http), "/api/auth/state", &[]).await;
    assert_eq!(st, 200, "this machine keeps HTTP");
    let (st, _, _) = plain(srv.http, "GET", &host, "/api/health", &[]).await;
    assert_eq!(st, 200, "supervision keeps HTTP");

    // En HTTPS, avec l'autorité seule : le nom du serveur et ceux des applis sont couverts.
    let (st, _, v) = send(tls(srv.https, &ca, "strata.test").await.unwrap(), "GET", &tls_host, "/api/https", &[("cookie", &srv.cookie)], None).await;
    assert_eq!(st, 200, "{v}");
    let v: Value = serde_json::from_str(&v).unwrap();
    assert_eq!(v["mode"], "strata");
    assert_eq!(v["listening"]["port"], srv.https);
    assert_eq!(v["cert"]["names"], json!(["strata.test", "*.strata.test"]));
    assert_eq!(v["cert"]["missing"], json!([]));
    assert!(tls(srv.https, &ca, "demo.strata.test").await.is_ok());
    assert!(tls(srv.https, &ca, "other.test").await.is_err());

    // La session, en HTTPS, est marquée Secure.
    let (_, h, _) = send(
        tls(srv.https, &ca, "strata.test").await.unwrap(),
        "POST",
        &tls_host,
        "/api/auth/login",
        &[("origin", &format!("https://{tls_host}"))],
        Some(json!({"username": "admin", "password": "password1"})),
    )
    .await;
    assert!(h.get("set-cookie").unwrap().to_str().unwrap().contains("; Secure"));

    // Un autre nom : un nouveau certificat, servi aussitôt, de la même autorité.
    let origin = format!("https://{tls_host}");
    let (st, _, v) = send(
        tls(srv.https, &ca, "strata.test").await.unwrap(),
        "PUT",
        &tls_host,
        "/api/server",
        &[("cookie", &srv.cookie), ("origin", &origin)],
        Some(json!({"name": "Régie B", "dnsName": "Other.Test", "timezone": "Europe/Paris"})),
    )
    .await;
    assert_eq!(st, 200, "{v}");
    let v: Value = serde_json::from_str(&v).unwrap();
    assert_eq!((v["name"].as_str(), v["dnsName"].as_str()), (Some("Régie B"), Some("other.test")));
    assert!(tls(srv.https, &ca, "demo.other.test").await.is_ok());
    let bad = send(
        tls(srv.https, &ca, "other.test").await.unwrap(),
        "PUT",
        "other.test",
        "/api/server",
        &[("cookie", &srv.cookie)],
        Some(json!({"name": "x", "dnsName": "single", "timezone": "Europe/Paris"})),
    )
    .await;
    assert_eq!((bad.0, bad.2.as_str()), (400, "not a DNS name: single"));

    // Retour au HTTP seul : l'écoute HTTPS s'arrête, le HTTP ne renvoie plus.
    let (st, _, _) = send(
        tls(srv.https, &ca, "other.test").await.unwrap(),
        "PUT",
        "other.test",
        "/api/https",
        &[("cookie", &srv.cookie)],
        Some(json!({"mode": "off", "port": srv.https})),
    )
    .await;
    assert_eq!(st, 200);
    assert!(tls(srv.https, &ca, "other.test").await.is_err());
    // (une adresse de l'API : l'interface web n'est pas construite en CI)
    let (st, _, _) = plain(srv.http, "GET", &format!("other.test:{}", srv.http), "/api/auth/state", &[]).await;
    assert_eq!(st, 200);

    // Le certificat d'entreprise ne se choisit pas avant d'être importé ; le port HTTP est pris.
    let (st, _, msg) = plain(srv.http, "PUT", "localhost", "/api/https", &[("cookie", &srv.cookie)]).await;
    assert_eq!(st, 415, "{msg}");
    let put = |body: Value| {
        let cookie = srv.cookie.clone();
        let port = srv.http;
        async move { send(TcpStream::connect(("127.0.0.1", port)).await.unwrap(), "PUT", "localhost", "/api/https", &[("cookie", &cookie)], Some(body)).await }
    };
    let (st, _, msg) = put(json!({"mode": "company", "port": srv.https})).await;
    assert_eq!((st, msg.as_str()), (400, "import the company certificate first"));
    let (st, _, msg) = put(json!({"mode": "strata", "port": srv.http})).await;
    assert_eq!((st, msg), (400, format!("port {} is already the HTTP one", srv.http)));

    // Redémarrer : le service s'arrête en disant qu'il faut se relancer.
    let (st, _, _) =
        send(TcpStream::connect(("127.0.0.1", srv.http)).await.unwrap(), "POST", "localhost", "/api/server/restart", &[("cookie", &srv.cookie)], None).await;
    assert_eq!(st, 202);
    let mut srv = srv;
    let ending = tokio::time::timeout(std::time::Duration::from_secs(10), srv.done.take().unwrap()).await.unwrap().unwrap().unwrap();
    assert_eq!(ending, Ending::Restart);
}
