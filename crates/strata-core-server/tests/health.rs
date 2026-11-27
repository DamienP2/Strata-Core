//! La santé de bout en bout : les ressources, le journal sur disque (avec les lignes d'une appli),
//! les incidents, l'export pour le support.
use axum::{
    body::Body,
    extract::connect_info::MockConnectInfo,
    http::{header, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::Value;
use std::net::SocketAddr;
use strata_core_server::{auth::NewUser, health, state::AppState};
use tower::ServiceExt;

async fn get(app: &Router, path: &str, cookie: &str) -> (StatusCode, Vec<u8>) {
    let req = Request::builder().uri(path).header(header::HOST, "strata.test").header(header::COOKIE, cookie).body(Body::empty()).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let st = res.status();
    (st, res.into_body().collect().await.unwrap().to_bytes().to_vec())
}

#[tokio::test]
async fn resources_journal_and_export() {
    let dir = std::env::temp_dir().join(format!("strata-health-it-{}", strata_core::new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    health::start(&dir);
    health::app_line("livecut", "2026-11-27 ERROR livecut: cannot open SDI 3");
    health::app_line("livecut", "2026-11-27 ERROR livecut: cannot open SDI 4");
    health::app_crash("livecut", "0.3.0", "code 3", 5, vec!["bye".into()]);
    let s = AppState::new(dir.clone());
    let admin = s.auth.add(NewUser { username: "admin".into(), display_name: String::new(), admin: true, password: Some("password1".into()) }).unwrap();
    let user = s.auth.add(NewUser { username: "ann".into(), display_name: String::new(), admin: false, password: Some("password1".into()) }).unwrap();
    let cookie = format!("strata_session={}", s.auth.open_session(&admin.id));
    let plain = format!("strata_session={}", s.auth.open_session(&user.id));
    let app = strata_core_server::app(s, None).layer(MockConnectInfo("10.0.0.5:5000".parse::<SocketAddr>().unwrap()));

    // Les ressources : la machine, le Core.
    let (st, b) = get(&app, "/api/health/resources", &cookie).await;
    assert_eq!(st, StatusCode::OK);
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert!(v["now"]["memTotal"].as_u64().unwrap() > 0);
    assert_eq!(v["now"]["uses"][0]["id"], "core");
    assert!(v["machine"]["cores"].as_u64().unwrap() >= 1);
    assert_eq!(get(&app, "/api/health/resources", &plain).await.0, StatusCode::FORBIDDEN);

    // Les incidents : le plantage de l'appli, ses deux erreurs comptées une fois.
    let (_, b) = get(&app, "/api/health/summary", &cookie).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert!(v["crashes"].as_u64().unwrap() >= 1);
    assert_eq!(v["unseenCrash"], true);
    assert_eq!(v["errors"], 2);
    let inc = v["incidents"].as_array().unwrap();
    assert_eq!((inc[0]["level"].as_str(), inc[0]["app"].as_str()), (Some("CRASH"), Some("livecut")));
    assert!(inc.iter().any(|i| i["level"] == "ERROR" && i["count"] == 2));

    // Le journal d'une appli.
    let (_, b) = get(&app, "/api/health/journal?level=error&cat=app:livecut", &cookie).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["lines"].as_array().unwrap().len(), 3);
    let (_, b) = get(&app, "/api/health/journal?level=error&cat=hardware", &cookie).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert!(v["lines"].as_array().unwrap().is_empty());

    // L'export : une archive .tar.gz.
    let (st, b) = get(&app, "/api/health/export", &cookie).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(&b[..2], &[0x1f, 0x8b]);
    let _ = std::fs::remove_dir_all(&dir);
}

async fn send(app: &Router, method: &str, path: &str, cookie: Option<&str>, body: Value) -> (StatusCode, Value) {
    // Sans session : un appel d'une appli, sur la machine.
    let host = if cookie.is_some() { "strata.test" } else { "127.0.0.1:8080" };
    let mut req = Request::builder().method(method).uri(path).header(header::HOST, host).header(header::CONTENT_TYPE, "application/json");
    if let Some(c) = cookie {
        req = req.header(header::COOKIE, c);
    }
    let res = app.clone().oneshot(req.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let st = res.status();
    let b = res.into_body().collect().await.unwrap().to_bytes();
    (st, serde_json::from_slice(&b).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&b).into())))
}

#[tokio::test]
async fn alert_settings_test_message_and_app_alerts() {
    // Un webhook qui garde ce qu'il reçoit.
    let hook = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", hook.local_addr().unwrap());
    let got = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));
    let got2 = got.clone();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut c, _) = hook.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let mut all = String::new();
        while !all.contains("\"server\"") {
            let n = c.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            all.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
        *got2.lock().await = all;
        c.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await.unwrap();
    });

    let dir = std::env::temp_dir().join(format!("strata-alerts-it-{}", strata_core::new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let s = AppState::new(dir.clone());
    let admin = s.auth.add(NewUser { username: "admin".into(), display_name: String::new(), admin: true, password: Some("password1".into()) }).unwrap();
    let cookie = format!("strata_session={}", s.auth.open_session(&admin.id));
    let c = Some(cookie.as_str());
    let app = strata_core_server::app(s.clone(), None);
    let remote = app.clone().layer(MockConnectInfo("10.0.0.5:5000".parse::<SocketAddr>().unwrap()));
    let local = app.layer(MockConnectInfo("127.0.0.1:5000".parse::<SocketAddr>().unwrap()));

    // Réglages : vérifiés, le mot de passe jamais renvoyé.
    let (st, msg) = send(&remote, "PUT", "/api/health/alerts", c, serde_json::json!({"webhook": "ftp://x"})).await;
    assert_eq!((st, msg.as_str()), (StatusCode::BAD_REQUEST, Some("the webhook address starts with https://")));
    let cfg = serde_json::json!({"enabled": true, "webhook": url, "emailTo": "regie@exemple.fr", "smtpHost": "smtp.exemple.fr", "smtpFrom": "strata@exemple.fr", "smtpUser": "u", "smtpPassword": "secret"});
    let (st, v) = send(&remote, "PUT", "/api/health/alerts", c, cfg).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["settings"]["smtpPassword"], "••••••••");
    assert!(std::fs::read_to_string(dir.join("alerts.json")).unwrap().contains("secret"));

    // Un message d'essai par le webhook (seul, sans e-mail).
    let (st, v) = send(&remote, "POST", "/api/health/alerts/test", c, serde_json::json!({"webhook": url})).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["results"], serde_json::json!([["webhook", null]]));
    assert!(got.lock().await.contains("Message d’essai des alertes de Strata Core"));

    // Les alertes des applis : depuis la machine, pour une appli active.
    let body = serde_json::json!({"app": "livecut", "key": "disk", "text": "plus de place pour enregistrer"});
    assert_eq!(send(&remote, "POST", "/api/health/alerts/app", None, body.clone()).await.0, StatusCode::FORBIDDEN);
    let (st, msg) = send(&local, "POST", "/api/health/alerts/app", None, body).await;
    assert_eq!((st, msg.as_str()), (StatusCode::BAD_REQUEST, Some("no active app: livecut")));
    let _ = std::fs::remove_dir_all(&dir);
}
