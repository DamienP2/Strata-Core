//! Le matériel de bout en bout, avec des cartes DeckLink simulées (`STRATA_FAKE_DECKLINK`) : ce que
//! voit la machine, les réglages (vérifiés), les réglages NDI partagés, ce que lisent les applis.
use axum::{
    body::Body,
    extract::connect_info::MockConnectInfo,
    http::{header, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::{net::SocketAddr, path::PathBuf};
use strata_core_server::{auth::NewUser, state::AppState};
use tower::ServiceExt;

struct Srv {
    dir: PathBuf,
    local: Router,
    remote: Router,
    cookie: String,
}

impl Drop for Srv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn server() -> Srv {
    // Ce binaire de test est à lui seul : la variable ne gêne aucun autre test.
    std::env::set_var("STRATA_FAKE_DECKLINK", "quad2,minimonitor");
    let dir = std::env::temp_dir().join(format!("strata-hw-{}", strata_core::new_id()));
    std::fs::create_dir_all(dir.join("shm")).unwrap();
    // Les entrées partagées : dans le dossier du test, pas dans /dev/shm.
    std::env::set_var("STRATA_SHARE_DIR", dir.join("shm"));
    // L'horloge : un PTP simulé, les fichiers du système écrits dans le dossier du test.
    std::env::set_var("STRATA_FAKE_CLOCK", "ptp");
    std::env::set_var("STRATA_FAKE_NICS", "connectx");
    std::env::set_var("STRATA_SYSTEM_ROOT", dir.join("root"));
    let s = AppState::new(dir.clone());
    let admin = s.auth.add(NewUser { username: "admin".into(), display_name: String::new(), admin: true, password: Some("password1".into()) }).unwrap();
    let cookie = format!("strata_session={}", s.auth.open_session(&admin.id));
    let app = strata_core_server::app(s, None);
    let lo: SocketAddr = "127.0.0.1:50000".parse().unwrap();
    let far: SocketAddr = "10.0.0.50:50000".parse().unwrap();
    Srv { local: app.clone().layer(MockConnectInfo(lo)), remote: app.layer(MockConnectInfo(far)), dir, cookie }
}

async fn call(app: &Router, method: &str, path: &str, host: &str, cookie: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path).header(header::HOST, host);
    if let Some(c) = cookie {
        req = req.header(header::COOKIE, c);
    }
    let req = match body {
        Some(b) => req.header(header::CONTENT_TYPE, "application/json").body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into())))
}

#[tokio::test]
async fn hardware_settings_and_what_apps_read() {
    let srv = server();
    let host = "strata.test";
    let c = Some(srv.cookie.as_str());

    // Ce que voit la machine : deux cartes simulées, leurs connecteurs, le gabarit des modèles.
    let (st, v) = call(&srv.remote, "GET", "/api/hardware", host, c, None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let cards = v["decklink"]["cards"].as_array().unwrap();
    assert_eq!(v["decklink"]["fake"], true);
    assert_eq!(cards.len(), 2);
    let quad = cards.iter().find(|c| c["modelName"] == "DeckLink Quad 2").unwrap();
    let mini = cards.iter().find(|c| c["modelName"] == "DeckLink Mini Monitor 4K").unwrap();
    assert_eq!(quad["connectors"], 8);
    assert_eq!(mini["ports"], json!(["out"]));
    assert!(v["models"].as_array().unwrap().iter().any(|m| m["name"] == "DeckLink 8K Pro"));
    assert!(v["nics"].is_array());
    assert_eq!(v["config"]["ptp"], json!({"domain": 127, "taiOffset": 37}));

    // Les réglages : SDI 1 en entrée, SDI 2 en sortie ; NDI ; un flux ST 2110.
    let quad_id = quad["id"].as_str().unwrap();
    let cfg = json!({
        "cards": [{"id": quad_id, "name": "Plateau", "connectors": [
            {"name": "Caméra 1", "function": "input"}, {"name": "Mélangeur clé 3", "function": "output"}
        ]}],
        "network": {"ndi": "lo"},
        "ndi": {"discovery": "10.0.0.2", "machine": "REGIE-1"},
        "flows": [{"name": "Régie 2", "direction": "input", "essence": "video", "red": "239.10.2.1:5004", "blue": "239.20.2.1:5004"}],
        "ptp": {"domain": 127, "taiOffset": 37},
    });
    let (st, v) = call(&srv.remote, "PUT", "/api/hardware", host, c, Some(cfg.clone())).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["config"]["flows"][0]["id"], "flow-1");
    assert_eq!(v["ndiChanged"], true);
    let ndi: Value = serde_json::from_str(&std::fs::read_to_string(srv.dir.join("ndi/ndi-config.v1.json")).unwrap()).unwrap();
    assert_eq!(ndi["ndi"]["networks"]["discovery"], "10.0.0.2");
    assert_eq!(ndi["ndi"]["machinename"], "REGIE-1");
    assert_eq!(ndi["ndi"]["adapters"]["allowed"][0], "127.0.0.1");

    // Refusés : une Mini Monitor en entrée, une adresse qui n'est pas multicast.
    let mut bad = cfg.clone();
    bad["cards"] = json!([{"id": mini["id"], "connectors": [{"function": "input"}]}]);
    let (st, msg) = call(&srv.remote, "PUT", "/api/hardware", host, c, Some(bad)).await;
    assert_eq!((st, msg.as_str().unwrap()), (StatusCode::BAD_REQUEST, "SDI 1 of DeckLink Mini Monitor 4K cannot be an input"));
    let mut bad = cfg;
    bad["flows"][0]["red"] = json!("10.0.0.1:5004");
    let (st, _) = call(&srv.remote, "PUT", "/api/hardware", host, c, Some(bad)).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // Ce que lisent les applis (depuis la machine, sans session) : l'appareil Desktop Video de
    // chaque connecteur. D'ailleurs, sans être administrateur : refusé.
    let (st, v) = call(&srv.local, "GET", "/api/hardware/shared", "127.0.0.1:8080", None, None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let card = &v["sdi"][0];
    assert_eq!((card["name"].as_str(), card["present"].as_bool()), (Some("Plateau"), Some(true)));
    let share = srv.dir.join("shm").join(format!("strata-sdi-{}-1", quad_id.to_ascii_lowercase()));
    assert_eq!(card["connectors"][0], json!({"number": 1, "name": "Caméra 1", "function": "input", "device": quad["devices"][0], "share": share}));
    assert_eq!(card["connectors"][1]["share"], Value::Null);
    assert_eq!(card["connectors"][1]["function"], "output");
    assert_eq!(v["network"]["ndi"]["ip"], "127.0.0.1");
    assert_eq!(v["flows"][0]["blue"], "239.20.2.1:5004");
    assert_eq!(v["network"]["mediaMode"], "redundant");
    assert!(v["ndi"]["configDir"].as_str().unwrap().ends_with("/ndi"));
    let (st, _) = call(&srv.remote, "GET", "/api/hardware/shared", host, None, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = call(&srv.remote, "GET", "/api/hardware/shared", host, c, None).await;
    assert_eq!(st, StatusCode::OK, "administrators too");

    // L'entrée est captée (mire simulée) : son état, son aperçu.
    let (st, v) = call(&srv.remote, "GET", "/api/hardware/inputs", host, c, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["inputs"].as_array().unwrap().len(), 1);
    assert_eq!((v["inputs"][0]["connector"].as_u64(), v["inputs"][0]["simulated"].as_bool()), (Some(1), Some(true)));
    let url = format!("/api/hardware/inputs/{quad_id}/1/preview.jpg");
    let mut st = StatusCode::NOT_FOUND;
    for _ in 0..100 {
        st = call(&srv.remote, "GET", &url, host, c, None).await.0;
        if st == StatusCode::OK {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    assert_eq!(st, StatusCode::OK);
    let (st, _) = call(&srv.remote, "GET", &format!("/api/hardware/inputs/{quad_id}/2/preview.jpg"), host, c, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "an output has no picture");
    // Plus d'entrée dans les réglages : la capture s'arrête, le partage disparaît.
    let (st, _) = call(
        &srv.remote,
        "PUT",
        "/api/hardware",
        host,
        c,
        Some(json!({"cards": [], "network": {"ndi": "lo"}, "ndi": {"discovery": "10.0.0.2", "machine": "REGIE-1"}})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(!share.exists());

    // L'horloge : PTP par ens2f0, NTP en secours ; enregistrer écrit les fichiers du système.
    let (_, h) = call(&srv.remote, "GET", "/api/hardware", host, c, None).await;
    let mut cfg = h["config"].clone();
    assert_eq!(cfg["clock"]["house"], json!({"rate": "25", "offset": "local", "fixedMinutes": 0, "dropFrame": true, "dailyJam": "00:00"}));
    cfg["network"]["ptp"] = json!("ens2f0");
    cfg["clock"]["ntpServers"] = json!([" 10.0.0.1 ", "NTP.regie.lan"]);
    let (st, v) = call(&srv.remote, "PUT", "/api/hardware", host, c, Some(cfg.clone())).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["config"]["clock"]["ntpServers"], json!(["10.0.0.1", "ntp.regie.lan"]));
    assert!(v["clockApplied"].as_array().unwrap().iter().all(|s| s["ok"] == true), "{v}");
    let chrony = std::fs::read_to_string(srv.dir.join("root/etc/chrony/conf.d/strata.conf")).unwrap();
    assert!(chrony.contains("refclock PHC /dev/ptp1 poll 0 dpoll -2 offset -37 prefer refid PTP"), "{chrony}");
    assert!(chrony.contains("server 10.0.0.1 iburst\nserver ntp.regie.lan iburst"));
    let ptp4l = std::fs::read_to_string(srv.dir.join("root/etc/strata/ptp4l.conf")).unwrap();
    assert!(ptp4l.contains("domainNumber            127") && ptp4l.ends_with("[ens2f0]\n"));
    assert!(srv.dir.join("root/etc/systemd/system/strata-ptp4l.service").exists());
    // Rien de changé pour l'heure : rien n'est réécrit.
    let (_, v) = call(&srv.remote, "PUT", "/api/hardware", host, c, Some(cfg.clone())).await;
    assert_eq!(v["clockApplied"], Value::Null);
    // NTP seulement : plus de PTP, ses fichiers sont retirés.
    let mut ntp_only = cfg.clone();
    ntp_only["clock"]["source"] = json!("ntp");
    let (st, _) = call(&srv.remote, "PUT", "/api/hardware", host, c, Some(ntp_only)).await;
    assert_eq!(st, StatusCode::OK);
    assert!(!srv.dir.join("root/etc/strata/ptp4l.conf").exists());
    assert!(!std::fs::read_to_string(srv.dir.join("root/etc/chrony/conf.d/strata.conf")).unwrap().contains("refclock"));
    let mut bad = cfg.clone();
    bad["clock"]["ntpServers"] = json!(["pas un serveur"]);
    let (st, msg) = call(&srv.remote, "PUT", "/api/hardware", host, c, Some(bad)).await;
    assert_eq!((st, msg.as_str().unwrap()), (StatusCode::BAD_REQUEST, "not an NTP server: pas un serveur"));
    let (st, _) = call(&srv.remote, "PUT", "/api/hardware", host, c, Some(cfg)).await;
    assert_eq!(st, StatusCode::OK);

    // L'état, le timecode maison, le bilan.
    let (st, v) = call(&srv.remote, "GET", "/api/hardware/clock", host, c, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["ptp"]["domain"], 127);
    assert_eq!(v["ntp"]["source"], "PTP");
    let tc = v["house"]["timecode"].as_str().unwrap();
    assert_eq!((tc.len(), v["house"]["fps"].as_f64()), (11, Some(25.0)), "{tc}");
    assert_eq!(v["timezone"], "Europe/Paris");
    assert_eq!(v["checks"][0]["code"], "timeFromPtp");
    assert_eq!(v["checks"][0]["level"], "ok");
    assert!(v["files"]["chrony"].as_str().unwrap().contains("refclock PHC"));
    let (st, v) = call(&srv.remote, "POST", "/api/hardware/clock/apply", host, c, Some(json!({}))).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["steps"].as_array().unwrap().len(), 3);

    // Les applis lisent le timecode maison.
    let (_, v) = call(&srv.local, "GET", "/api/hardware/shared", "127.0.0.1:8080", None, None).await;
    assert_eq!(v["clock"]["house"]["rate"], "25");
    assert_eq!(v["clock"]["timezone"], "Europe/Paris");
}
