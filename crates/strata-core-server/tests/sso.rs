//! Le SSO de bout en bout, avec un fournisseur d'identité simulé (découverte, clés, jeton signé ES256)
//! servi sur 127.0.0.1 : départ, retour, vérification du jeton, compte créé avec ses groupes et son
//! rôle, refus (aucun groupe, état d'un autre navigateur), comptes locaux.
use axum::{
    body::Body,
    extract::{connect_info::MockConnectInfo, State},
    http::{header, Request, StatusCode},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use http_body_util::BodyExt;
use ring::{
    rand::SystemRandom,
    signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING},
};
use serde_json::{json, Value};
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use strata_core_server::state::AppState;
use tower::ServiceExt;

const HOST: &str = "strata01.regie.exemple.fr";
const CLIENT: &str = "strata-core";

// ---------------------------------------------------------------- le fournisseur simulé

struct Idp {
    base: String,
    key: EcdsaKeyPair,
    /// Les revendications du prochain jeton (le nonce y est mis par le test).
    claims: Mutex<Value>,
}

type IdpState = Arc<Idp>;

async fn discovery(State(i): State<IdpState>) -> Json<Value> {
    Json(json!({
        "issuer": i.base,
        "authorization_endpoint": format!("{}/authorize", i.base),
        "token_endpoint": format!("{}/token", i.base),
        "jwks_uri": format!("{}/jwks", i.base),
    }))
}

async fn keys(State(i): State<IdpState>) -> Json<Value> {
    let p = i.key.public_key().as_ref();
    Json(json!({"keys": [{"kty": "EC", "crv": "P-256", "kid": "k1", "x": URL_SAFE_NO_PAD.encode(&p[1..33]), "y": URL_SAFE_NO_PAD.encode(&p[33..65])}]}))
}

async fn token(State(i): State<IdpState>, body: String) -> Json<Value> {
    assert!(body.contains("grant_type=authorization_code") && body.contains("code=good-code") && body.contains("code_verifier="), "{body}");
    let head = URL_SAFE_NO_PAD.encode(json!({"alg": "ES256", "kid": "k1", "typ": "JWT"}).to_string());
    let claims = URL_SAFE_NO_PAD.encode(i.claims.lock().unwrap().to_string());
    let msg = format!("{head}.{claims}");
    let sig = i.key.sign(&SystemRandom::new(), msg.as_bytes()).unwrap();
    Json(json!({"id_token": format!("{msg}.{}", URL_SAFE_NO_PAD.encode(sig.as_ref())), "token_type": "Bearer"}))
}

async fn idp() -> IdpState {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let rng = SystemRandom::new();
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
    let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng).unwrap();
    let i = Arc::new(Idp { base, key, claims: Mutex::new(Value::Null) });
    let app =
        Router::new().route("/.well-known/openid-configuration", get(discovery)).route("/jwks", get(keys)).route("/token", post(token)).with_state(i.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    i
}

// ---------------------------------------------------------------- le Core

struct Srv {
    dir: PathBuf,
    app: Router,
}

impl Drop for Srv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct Answer {
    status: StatusCode,
    location: Option<String>,
    cookies: Vec<String>,
    body: Value,
}

impl Srv {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("strata-sso-{}", strata_core::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), format!(r#"{{"name":"Régie A","dnsName":"{HOST}","https":{{"mode":"proxy"}}}}"#)).unwrap();
        let far: SocketAddr = "10.0.0.50:50000".parse().unwrap();
        let app = strata_core_server::app(AppState::new(dir.clone()), None).layer(MockConnectInfo(far));
        Srv { dir, app }
    }

    async fn call(&self, method: &str, path: &str, cookie: Option<&str>, body: Option<Value>) -> Answer {
        let mut req = Request::builder().method(method).uri(path).header(header::HOST, HOST);
        if let Some(c) = cookie {
            req = req.header(header::COOKIE, c);
        }
        let req = match body {
            Some(b) => req.header(header::CONTENT_TYPE, "application/json").body(Body::from(b.to_string())).unwrap(),
            None => req.body(Body::empty()).unwrap(),
        };
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let location = res.headers().get(header::LOCATION).map(|v| v.to_str().unwrap().to_string());
        let cookies = res.headers().get_all(header::SET_COOKIE).iter().map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string()).collect();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let body = serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()));
        Answer { status, location, cookies, body }
    }

    /// Le premier administrateur, créé comme par la ligne de commande.
    fn admin(&self) -> strata_core_server::auth::Auth {
        let a = strata_core_server::auth::Auth::load(&self.dir);
        a.add(strata_core_server::auth::NewUser { username: "damien".into(), display_name: "Damien".into(), admin: true, password: Some("password1".into()) })
            .unwrap();
        a
    }
}

fn param(url: &str, name: &str) -> String {
    url.split(['?', '&']).find_map(|kv| kv.strip_prefix(&format!("{name}="))).unwrap_or_default().to_string()
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

/// Un aller-retour complet : départ, jeton préparé avec le nonce, retour. Rend la réponse du retour.
async fn sign_in(srv: &Srv, idp: &Idp, sub: &str, groups: &[&str]) -> Answer {
    let start = srv.call("GET", "/api/auth/sso/start?next=/reglages/utilisateurs", None, None).await;
    assert_eq!(start.status, StatusCode::SEE_OTHER, "{:?}", start.body);
    let to = start.location.unwrap();
    assert!(to.starts_with(&format!("{}/authorize?", idp.base)), "{to}");
    assert!(to.contains("code_challenge_method=S256") && to.contains(&format!("client_id={CLIENT}")));
    assert!(to.contains(&format!("redirect_uri=http%3A%2F%2F{HOST}%2Fapi%2Fauth%2Fsso%2Fcallback")), "{to}");
    let (state, nonce) = (param(&to, "state"), param(&to, "nonce"));
    let browser = start.cookies.iter().find(|c| c.starts_with("strata_sso=")).unwrap().clone();
    *idp.claims.lock().unwrap() = json!({
        "iss": idp.base, "aud": CLIENT, "exp": now() + 300, "nonce": nonce, "sub": sub,
        "email": format!("{sub}@exemple.fr"), "email_verified": true, "name": format!("Nom {sub}"), "groups": groups,
    });
    srv.call("GET", &format!("/api/auth/sso/callback?code=good-code&state={state}"), Some(&browser), None).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn directory_sign_in() {
    let idp = idp().await;
    let srv = Srv::new();
    srv.admin();
    let admin = srv.call("POST", "/api/auth/login", None, Some(json!({"username": "damien", "password": "password1"}))).await;
    let admin = admin.cookies[0].clone();

    // Un groupe du Core rempli par un groupe de l'annuaire
    let g = srv.call("POST", "/api/groups", Some(&admin), Some(json!({"name": "Réalisation", "directory": ["/Realisateurs"]}))).await;
    assert_eq!(g.status, StatusCode::OK, "{:?}", g.body);
    let real = g.body["id"].as_str().unwrap().to_string();

    // Réglages : secret masqué à la relecture, gardé quand on renvoie le masque
    let set = json!({"enabled": true, "provider": "okta", "issuer": idp.base, "clientId": CLIENT, "clientSecret": "s3cret", "adminGroups": ["it-admins"]});
    let r = srv.call("PUT", "/api/sso", Some(&admin), Some(set)).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["sso"]["clientSecret"], "••••••••");
    assert_eq!(r.body["redirectUri"], format!("https://{HOST}/api/auth/sso/callback"));
    let mut again = r.body["sso"].clone();
    again["domains"] = json!([]);
    let r = srv.call("PUT", "/api/sso", Some(&admin), Some(again)).await;
    assert_eq!(r.status, StatusCode::OK);
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(srv.dir.join("config.json")).unwrap()).unwrap();
    assert_eq!(saved["sso"]["clientSecret"], "s3cret", "the mask keeps the secret");
    let r = srv.call("POST", "/api/sso/test", Some(&admin), Some(json!({"issuer": idp.base}))).await;
    assert_eq!((r.status, r.body["keys"].as_u64()), (StatusCode::OK, Some(1)));
    let st = srv.call("GET", "/api/auth/state", None, None).await;
    assert_eq!((st.body["sso"].as_str(), st.body["localAccounts"].as_str()), (Some("Okta"), Some("adminOnly")));

    // Une réalisatrice : compte créé, dans son groupe, sans l'administration, renvoyée où elle allait
    let back = sign_in(&srv, &idp, "ann", &["/REALISATEURS", "autre"]).await;
    assert_eq!(back.status, StatusCode::SEE_OTHER, "{:?}", back.location);
    assert_eq!(back.location.as_deref(), Some("/reglages/utilisateurs"));
    let session = back.cookies.iter().find(|c| c.starts_with("strata_session=")).unwrap().clone();
    let me = srv.call("GET", "/api/auth/state", Some(&session), None).await.body["user"].clone();
    assert_eq!(me["username"], "ann@exemple.fr");
    assert_eq!(me["displayName"], "Nom ann");
    assert_eq!((me["sso"].as_bool(), me["admin"].as_bool()), (Some(true), Some(false)));
    assert_eq!(me["groups"], json!([real]));
    assert_eq!(me["groupNames"], json!(["Réalisation"]));

    // Un administrateur de l'annuaire
    let back = sign_in(&srv, &idp, "it", &["IT-Admins"]).await;
    let session = back.cookies.iter().find(|c| c.starts_with("strata_session=")).unwrap().clone();
    assert_eq!(srv.call("GET", "/api/auth/state", Some(&session), None).await.body["user"]["admin"], true);

    // Personne dans aucun groupe connu : refusé
    let back = sign_in(&srv, &idp, "bob", &["compta"]).await;
    assert_eq!(back.location.as_deref(), Some("/?sso_error=noGroup&sso_detail="));

    // Un retour sans le cookie du navigateur qui a commencé : refusé
    let start = srv.call("GET", "/api/auth/sso/start", None, None).await;
    let state = param(start.location.as_deref().unwrap(), "state");
    let back = srv.call("GET", &format!("/api/auth/sso/callback?code=good-code&state={state}"), Some("strata_sso=autre"), None).await;
    assert_eq!(back.location.as_deref(), Some("/?sso_error=expired&sso_detail="));

    // Comptes locaux : administrateurs seulement
    srv.call("POST", "/api/users", Some(&admin), Some(json!({"username": "claire", "password": "password1"}))).await;
    let r = srv.call("POST", "/api/auth/login", None, Some(json!({"username": "claire", "password": "password1"}))).await;
    assert_eq!((r.status, r.body.as_str()), (StatusCode::FORBIDDEN, Some("sign in with Okta")));
    let r = srv.call("POST", "/api/auth/login", None, Some(json!({"username": "damien", "password": "password1"}))).await;
    assert_eq!(r.status, StatusCode::OK);

    // Un utilisateur n'a pas accès aux réglages du SSO
    let ann = sign_in(&srv, &idp, "ann", &["/Realisateurs"]).await;
    let ann = ann.cookies.iter().find(|c| c.starts_with("strata_session=")).unwrap().clone();
    assert_eq!(srv.call("GET", "/api/sso", Some(&ann), None).await.status, StatusCode::FORBIDDEN);
}
