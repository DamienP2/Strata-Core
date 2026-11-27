//! Plusieurs serveurs de bout en bout, sur de vraies écoutes : invitation, rattachement, battement,
//! cockpit du maître avec les applis du membre, groupes du maître sur le membre, connexion unique
//! (jeton signé, une fois), détacher.
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use serde_json::{json, Value};
use strata_core_server::{auth::NewUser, state::AppState};

struct Srv {
    dir: PathBuf,
    url: String,
    s: strata_core_server::state::Shared,
    admin: String,
}

impl Drop for Srv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn start(name: &str) -> Srv {
    let dir = std::env::temp_dir().join(format!("strata-cluster-it-{}", strata_core::new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let s = AppState::new(dir.clone());
    let mut cfg = (**s.config.load()).clone();
    cfg.name = name.into();
    cfg.dns_name = "localhost".into();
    s.config.store(Arc::new(cfg));
    let admin = s
        .auth
        .add(NewUser {
            username: format!("admin-{}", name.chars().last().unwrap().to_ascii_lowercase()),
            display_name: String::new(),
            admin: true,
            password: Some("password1".into()),
        })
        .unwrap();
    let admin = s.auth.open_session(&admin.id);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(strata_core_server::serve_on(s.clone(), listener, None));
    Srv { dir, url: format!("http://127.0.0.1:{}", addr.port()), s, admin }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().max_redirects(0).http_status_as_error(false).timeout_global(Some(Duration::from_secs(10))).build().into()
}

/// Une requête : (statut, en-têtes utiles, corps).
async fn call(method: &'static str, url: String, cookie: Option<String>, body: Option<Value>) -> (u16, Option<String>, Vec<String>, Value) {
    tokio::task::spawn_blocking(move || {
        let a = agent();
        let res = match (method, body) {
            ("GET", _) => {
                let mut r = a.get(&url);
                if let Some(c) = &cookie {
                    r = r.header("cookie", c);
                }
                r.call()
            }
            (m, b) => {
                let mut r = if m == "DELETE" { a.delete(&url).force_send_body() } else { a.post(&url) };
                if let Some(c) = &cookie {
                    r = r.header("cookie", c);
                }
                r.header("content-type", "application/json").send(b.unwrap_or(json!({})).to_string())
            }
        }
        .unwrap();
        let status = res.status().as_u16();
        let location = res.headers().get("location").and_then(|v| v.to_str().ok()).map(str::to_string);
        let cookies = res.headers().get_all("set-cookie").iter().filter_map(|v| v.to_str().ok()).map(str::to_string).collect();
        let text = res.into_body().read_to_string().unwrap_or_default();
        (status, location, cookies, serde_json::from_str(&text).unwrap_or(Value::String(text)))
    })
    .await
    .unwrap()
}

fn cookie_of(set: &[String], name: &str) -> Option<String> {
    set.iter().find(|c| c.starts_with(&format!("{name}="))).map(|c| c.split(';').next().unwrap().to_string())
}

#[tokio::test(flavor = "multi_thread")]
async fn two_servers_one_cockpit() {
    let master = start("Régie A").await;
    let member = start("Régie B").await;
    let mc = format!("strata_session={}", master.admin);
    let bc = format!("strata_session={}", member.admin);

    // Une appli sur le membre ; un groupe et un utilisateur sur le maître.
    let d = member.dir.join("apps/livecut");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("strata-app.toml"), "id = \"livecut\"\nname = \"LiveCut\"\nversion = \"1.0.0\"\nport = 18101\n\n[requires]\ncore = \">=0.1\"\n")
        .unwrap();
    member.s.apps.set_active("livecut", true).unwrap();
    let group = master.s.groups.add("Réalisation", vec![]).unwrap();
    let ann = master.s.auth.add(NewUser { username: "ann".into(), display_name: "Ann Lévy".into(), admin: false, password: Some("password1".into()) }).unwrap();
    master
        .s
        .auth
        .modify(&ann.id, |u| {
            u.groups = vec![group.id.clone()];
            Ok(())
        })
        .unwrap();
    let ann_cookie = format!("strata_session={}", master.s.auth.open_session(&ann.id));

    // Un mauvais code, puis le bon.
    let (st, _, _, v) = call("POST", format!("{}/api/cluster/invite", master.url), Some(mc.clone()), None).await;
    assert_eq!(st, 200, "{v}");
    let code = v["code"].as_str().unwrap().to_string();
    let (st, _, _, v) =
        call("POST", format!("{}/api/cluster/join", member.url), Some(bc.clone()), Some(json!({"masterUrl": master.url, "code": "AAAA-BBBB-CC"}))).await;
    assert_eq!((st, v.as_str()), (400, Some("this code is not valid (wrong, used or expired)")));
    let (st, _, _, v) =
        call("POST", format!("{}/api/cluster/join", member.url), Some(bc.clone()), Some(json!({"masterUrl": master.url, "code": code.to_lowercase()}))).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["master"]["name"], "Régie A");
    // Le code ne sert qu'une fois.
    let (st, _, _, _) = call("POST", format!("{}/api/cluster/join", member.url), Some(bc.clone()), Some(json!({"masterUrl": master.url, "code": code}))).await;
    assert_eq!(st, 400);

    // Le battement : le maître voit le membre et son appli ; le membre a les groupes du maître.
    let mut cockpit = Value::Null;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        cockpit = call("GET", format!("{}/api/cockpit", master.url), Some(mc.clone()), None).await.3;
        if cockpit["servers"].as_array().is_some_and(|s| s.len() == 2 && s[1]["up"] == true) {
            break;
        }
    }
    let servers = cockpit["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 2, "{cockpit}");
    assert_eq!((servers[1]["name"].as_str(), servers[1]["up"].as_bool()), (Some("Régie B"), Some(true)));
    assert!(cockpit["apps"].as_array().unwrap().iter().any(|a| a["id"] == "livecut"));
    let inst = cockpit["instances"].as_array().unwrap().iter().find(|i| i["app"] == "livecut").unwrap().clone();
    assert_eq!(inst["server"], servers[1]["id"]);
    assert!(inst["url"].as_str().unwrap().starts_with("http://livecut.localhost:"), "{inst}");
    assert!(member.s.groups.exists(&group.id));
    let (_, _, _, v) = call("GET", format!("{}/api/cluster", master.url), Some(mc.clone()), None).await;
    assert_eq!(v["members"][0]["apps"], 1);
    let (_, _, _, v) = call("GET", format!("{}/api/auth/state", member.url), None, None).await;
    assert_eq!((v["cluster"]["masterName"].as_str(), v["cluster"]["masterUp"].as_bool()), (Some("Régie A"), Some(true)));

    // La connexion unique : le membre renvoie au maître, qui revient avec un jeton.
    let (st, loc, set, _) = call("GET", format!("{}/api/cluster/login?next=%2Freglages", member.url), None, None).await;
    assert_eq!(st, 303);
    let to_master = loc.unwrap();
    assert!(to_master.starts_with(&format!("{}/api/cluster/authorize?", master.url)), "{to_master}");
    let state = cookie_of(&set, "strata_cluster_state").unwrap();
    // Sans session sur le maître : sa connexion d'abord.
    let (st, loc, _, _) = call("GET", to_master.clone(), None, None).await;
    assert_eq!(st, 303);
    assert!(loc.unwrap().contains("?next=%2Fapi%2Fcluster%2Fauthorize"));
    let (st, loc, _, _) = call("GET", to_master, Some(ann_cookie), None).await;
    assert_eq!(st, 303);
    // Le membre est connu par son nom DNS (localhost ici) et son port.
    let back = loc.unwrap().replace("://localhost:", "://127.0.0.1:");
    assert!(back.starts_with(&format!("{}/api/cluster/callback?token=", member.url)), "{back}");
    let (st, loc, set, v) = call("GET", back.clone(), Some(state.clone()), None).await;
    assert_eq!(st, 303, "{v}");
    assert_eq!(loc.as_deref(), Some("/reglages"));
    let session = cookie_of(&set, "strata_session").unwrap();
    let (_, _, _, v) = call("GET", format!("{}/api/auth/state", member.url), Some(session), None).await;
    assert_eq!((v["user"]["displayName"].as_str(), v["user"]["admin"].as_bool()), (Some("Ann Lévy"), Some(false)));
    assert_eq!(v["user"]["groupNames"], json!(["Réalisation"]));
    // Un jeton ne sert qu'une fois ; pas sans le cookie de la demande.
    assert_eq!(call("GET", back.clone(), Some(state), None).await.0, 403);
    assert_eq!(call("GET", back, None, None).await.0, 403);

    // Détaché depuis le maître : le membre redevient autonome.
    let id = servers[1]["id"].as_str().unwrap().to_string();
    let (st, _, _, _) = call("DELETE", format!("{}/api/cluster/members/{id}", master.url), Some(mc.clone()), None).await;
    assert_eq!(st, 200);
    let mut role = Value::Null;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        role = call("GET", format!("{}/api/cluster", member.url), Some(bc.clone()), None).await.3["role"].clone();
        if role == "master" {
            break;
        }
    }
    assert_eq!(role, "master");
    let (_, _, _, v) = call("GET", format!("{}/api/cockpit", master.url), Some(mc), None).await;
    assert_eq!(v["servers"].as_array().unwrap().len(), 1);
}
