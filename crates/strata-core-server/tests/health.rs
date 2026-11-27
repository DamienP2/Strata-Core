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
    assert_eq!(v["crashes"], 1);
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
