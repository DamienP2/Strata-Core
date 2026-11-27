//! Le HTTPS du Core : son certificat, son écoute, et le passage du HTTP au HTTPS.
//!
//! - **Fait par Strata** : le Core crée une fois l'autorité de ce serveur (« Strata CA · Régie A »,
//!   10 ans), puis un certificat pour `<nom DNS>` et `*.<nom DNS>` (397 jours), renouvelé tout seul
//!   30 jours avant sa fin ou quand le nom change. On installe l'autorité une fois sur chaque poste
//!   (`/api/https/ca.crt`) : plus aucun avertissement, pour le cockpit comme pour les applis.
//! - **Entreprise** : un certificat importé (sa chaîne et sa clé privée), qui doit couvrir les mêmes
//!   noms.
//!
//! L'écoute HTTPS démarre, change de port ou de certificat sans redémarrer le Core. Quand elle tourne,
//! le HTTP renvoie vers le HTTPS (sauf `/api/health`, l'autorité à télécharger, et les requêtes de la
//! machine elle-même par `localhost`).
//! Fichiers : `<données>/tls/` (clés privées lisibles par le seul service).
use arc_swap::ArcSwapOption;
use axum::{
    body::Body,
    extract::{ConnectInfo, Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
    Router,
};
use hyper_util::rt::TokioIo;
use rustls::{
    pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer},
    server::{ClientHello, ResolvesServerCert},
    sign::CertifiedKey,
};
use serde::Serialize;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::task::JoinHandle;
use tower::ServiceExt;

use crate::{config::HttpsMode, state::Shared};

const CA_YEARS: i64 = 10;
/// Ce que les navigateurs acceptent au plus pour un certificat (398 jours).
const LEAF_DAYS: i64 = 397;
const RENEW_DAYS: i64 = 30;

/// Ce qu'on sait d'un certificat.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CertInfo {
    /// `strata` ou `company`.
    pub origin: &'static str,
    pub subject: String,
    pub issuer: String,
    /// Les noms qu'il couvre.
    pub names: Vec<String>,
    /// Secondes Unix.
    pub not_before: i64,
    pub not_after: i64,
    /// Les noms du serveur qu'il ne couvre pas (`strata01.regie.fr`, `*.strata01.regie.fr`).
    pub missing: Vec<String>,
}

/// L'état de l'écoute HTTPS.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Listening {
    /// Le port où le Core écoute en HTTPS, s'il écoute.
    pub port: Option<u16>,
    /// Pourquoi il n'écoute pas, s'il devrait.
    pub error: Option<String>,
}

/// Une requête arrivée en HTTPS.
#[derive(Clone, Copy)]
pub struct OverTls;

#[derive(Debug)]
struct Resolver(Arc<ArcSwapOption<CertifiedKey>>);

impl ResolvesServerCert for Resolver {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.0.load_full()
    }
}

pub struct Tls {
    dir: PathBuf,
    current: Arc<ArcSwapOption<CertifiedKey>>,
    info: Mutex<Option<CertInfo>>,
    listener: Mutex<Option<(SocketAddr, JoinHandle<()>)>>,
    status: Mutex<Listening>,
    router: OnceLock<Router>,
    /// L'adresse d'écoute HTTP (son adresse IP sert aussi au HTTPS).
    http: Mutex<Option<SocketAddr>>,
    /// Une seule mise en accord à la fois.
    applying: tokio::sync::Mutex<()>,
}

impl Tls {
    pub fn new(data_dir: &Path) -> Self {
        Tls {
            dir: data_dir.join("tls"),
            current: Arc::default(),
            info: Mutex::default(),
            listener: Mutex::default(),
            status: Mutex::default(),
            router: OnceLock::new(),
            http: Mutex::default(),
            applying: tokio::sync::Mutex::new(()),
        }
    }

    /// Ce que sert l'écoute HTTPS, et où écoute le HTTP.
    pub fn set_router(&self, router: Router, http: SocketAddr) {
        let _ = self.router.set(router);
        *self.http.lock().unwrap() = Some(http);
    }

    pub fn http_addr(&self) -> Option<SocketAddr> {
        *self.http.lock().unwrap()
    }

    pub fn listening(&self) -> Listening {
        self.status.lock().unwrap().clone()
    }

    /// Le certificat servi.
    pub fn info(&self) -> Option<CertInfo> {
        self.info.lock().unwrap().clone()
    }

    /// L'autorité de ce serveur, à installer sur les postes (si Strata l'a faite).
    pub fn ca_pem(&self) -> Option<String> {
        std::fs::read_to_string(self.dir.join("strata-ca.crt")).ok()
    }

    /// Le certificat d'entreprise importé, s'il y en a un.
    pub fn company(&self, dns: &str) -> Option<CertInfo> {
        load(&self.dir, "company", dns).ok().map(|(_, i)| i)
    }

    fn stop(&self) {
        if let Some((_, h)) = self.listener.lock().unwrap().take() {
            h.abort();
        }
        self.status.lock().unwrap().port = None;
    }
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

/// Les noms qu'un certificat de ce serveur doit couvrir.
pub fn needed_names(dns: &str) -> Vec<String> {
    vec![dns.to_string(), format!("*.{dns}")]
}

/// Un nom est-il couvert (exactement, ou par un `*.` du niveau au-dessus) ?
fn covered(name: &str, names: &[String]) -> bool {
    names.iter().any(|n| {
        let n = n.to_ascii_lowercase();
        n == name || (!name.starts_with("*.") && n.strip_prefix("*.").is_some_and(|parent| name.split_once('.').is_some_and(|(_, rest)| rest == parent)))
    })
}

/// Ce que dit le premier certificat d'une chaîne.
fn describe(der: &[u8], origin: &'static str, dns: &str) -> Result<CertInfo, String> {
    let (_, cert) = x509_parser::parse_x509_certificate(der).map_err(|_| "the certificate is unreadable".to_string())?;
    let mut names = Vec::new();
    if let Ok(Some(san)) = cert.subject_alternative_name() {
        for n in &san.value.general_names {
            if let x509_parser::extensions::GeneralName::DNSName(d) = n {
                names.push(d.to_string());
            }
        }
    }
    let missing = needed_names(dns).into_iter().filter(|n| !covered(n, &names)).collect();
    Ok(CertInfo {
        origin,
        subject: cert.subject().to_string(),
        issuer: cert.issuer().to_string(),
        names,
        not_before: cert.validity().not_before.timestamp(),
        not_after: cert.validity().not_after.timestamp(),
        missing,
    })
}

/// Une chaîne (PEM) et sa clé privée (PEM), prêtes à servir.
fn certified(chain: &str, key: &str) -> Result<CertifiedKey, String> {
    let certs: Vec<CertificateDer<'static>> =
        CertificateDer::pem_slice_iter(chain.as_bytes()).collect::<Result<_, _>>().map_err(|_| "the certificate file is unreadable".to_string())?;
    if certs.is_empty() {
        return Err("there is no certificate in the file".into());
    }
    let key = PrivateKeyDer::from_pem_slice(key.as_bytes()).map_err(|_| "there is no private key in the file".to_string())?;
    let signing = rustls::crypto::ring::sign::any_supported_type(&key).map_err(|_| "this kind of private key is not supported".to_string())?;
    let ck = CertifiedKey::new(certs, signing);
    ck.keys_match().map_err(|_| "the private key does not match the certificate".to_string())?;
    Ok(ck)
}

/// Lit un certificat gardé (`strata` ou `company`).
fn load(dir: &Path, origin: &'static str, dns: &str) -> Result<(CertifiedKey, CertInfo), String> {
    let chain = std::fs::read_to_string(dir.join(format!("{origin}.crt"))).map_err(|_| format!("no {origin} certificate"))?;
    let key = std::fs::read_to_string(dir.join(format!("{origin}.key"))).map_err(|_| format!("no {origin} key"))?;
    let ck = certified(&chain, &key)?;
    let info = describe(&ck.cert[0], origin, dns)?;
    Ok((ck, info))
}

fn write(dir: &Path, name: &str, text: &str, private: bool) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    strata_core::files::write_atomic(&dir.join(name), text.as_bytes(), private).map_err(|e| format!("cannot write {name}: {e}"))
}

/// Le certificat fait par Strata : l'autorité du serveur (créée une fois), puis le certificat du nom
/// DNS, refait s'il manque, change de nom, ou finit dans moins de 30 jours.
pub fn ensure_strata(dir: &Path, dns: &str, server_name: &str) -> Result<(CertifiedKey, CertInfo), String> {
    use rcgen::{BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose};
    let err = |e: rcgen::Error| format!("cannot make the certificate: {e}");
    let day = time::Duration::days(1);
    let now = time::OffsetDateTime::now_utc();

    let (ca_crt, ca_key) = (dir.join("strata-ca.crt"), dir.join("strata-ca.key"));
    if !ca_crt.exists() || !ca_key.exists() {
        let key = KeyPair::generate().map_err(err)?;
        let mut p = CertificateParams::new(Vec::<String>::new()).map_err(err)?;
        p.distinguished_name.push(DnType::CommonName, format!("Strata CA · {server_name}"));
        p.distinguished_name.push(DnType::OrganizationName, "Strata");
        p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
        p.not_before = now - day;
        p.not_after = now + time::Duration::days(365 * CA_YEARS);
        let cert = p.self_signed(&key).map_err(err)?;
        write(dir, "strata-ca.key", &key.serialize_pem(), true)?;
        write(dir, "strata-ca.crt", &cert.pem(), false)?;
        tracing::info!("HTTPS: the authority of this server was made");
    }
    if let Ok((ck, info)) = load(dir, "strata", dns) {
        if info.missing.is_empty() && info.not_after - now.unix_timestamp() > RENEW_DAYS * 86400 {
            return Ok((ck, info));
        }
    }
    let ca_pem = std::fs::read_to_string(&ca_crt).map_err(|e| format!("cannot read the authority: {e}"))?;
    let ca_key = KeyPair::from_pem(&std::fs::read_to_string(&ca_key).map_err(|e| format!("cannot read the authority's key: {e}"))?).map_err(err)?;
    let issuer = Issuer::from_ca_cert_pem(&ca_pem, ca_key).map_err(err)?;
    let key = KeyPair::generate().map_err(err)?;
    let mut p = CertificateParams::new(needed_names(dns)).map_err(err)?;
    p.distinguished_name.push(DnType::CommonName, dns);
    p.distinguished_name.push(DnType::OrganizationName, "Strata");
    p.key_usages = vec![KeyUsagePurpose::DigitalSignature, KeyUsagePurpose::KeyEncipherment];
    p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    p.use_authority_key_identifier_extension = true;
    p.not_before = now - day;
    p.not_after = now + time::Duration::days(LEAF_DAYS);
    let cert = p.signed_by(&key, &issuer).map_err(err)?;
    write(dir, "strata.key", &key.serialize_pem(), true)?;
    write(dir, "strata.crt", &format!("{}{ca_pem}", cert.pem()), false)?;
    tracing::info!("HTTPS: certificate made for {dns} and *.{dns}");
    load(dir, "strata", dns)
}

/// Importe le certificat de l'entreprise : sa chaîne et sa clé, vérifiées (clé assortie, pas
/// expiré, noms du serveur couverts).
pub fn import(dir: &Path, chain: &str, key: &str, dns: &str) -> Result<CertInfo, String> {
    let ck = certified(chain, key)?;
    let info = describe(&ck.cert[0], "company", dns)?;
    let date = |t: i64| time::OffsetDateTime::from_unix_timestamp(t).map(|d| d.date().to_string()).unwrap_or_default();
    if info.not_after < now() {
        return Err(format!("the certificate expired on {}", date(info.not_after)));
    }
    if info.not_before > now() {
        return Err(format!("the certificate is valid only from {}", date(info.not_before)));
    }
    if !info.missing.is_empty() {
        return Err(format!("the certificate does not cover {}", info.missing.join(", ")));
    }
    write(dir, "company.key", key, true)?;
    write(dir, "company.crt", chain, false)?;
    Ok(info)
}

/// Met le HTTPS en accord avec les réglages : le certificat, puis l'écoute (démarrée, déplacée ou
/// arrêtée). Appelé au démarrage, à chaque changement, et deux fois par jour (renouvellement).
pub async fn apply(s: &Shared) {
    let _one = s.tls.applying.lock().await;
    let cfg = s.config.load_full();
    let dir = s.tls.dir.clone();
    let (dns, name, mode) = (cfg.dns_name.clone(), cfg.name.clone(), cfg.https.mode);
    let cert = tokio::task::spawn_blocking(move || match mode {
        HttpsMode::Strata => ensure_strata(&dir, &dns, &name).map(Some),
        HttpsMode::Company => load(&dir, "company", &dns).map(Some),
        _ => Ok(None),
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    let cert = match cert {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("HTTPS: {e}");
            s.tls.stop();
            s.tls.status.lock().unwrap().error = Some(e);
            return;
        }
    };
    *s.tls.info.lock().unwrap() = cert.as_ref().map(|(_, i)| i.clone());
    s.tls.current.store(cert.map(|(ck, _)| Arc::new(ck)));
    if !cfg.https.served() {
        s.tls.stop();
        s.tls.status.lock().unwrap().error = None;
        return;
    }
    let ip = s.tls.http_addr().map(|a| a.ip()).unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    let addr = SocketAddr::new(ip, cfg.https.port);
    if s.tls.listener.lock().unwrap().as_ref().is_some_and(|(a, h)| *a == addr && !h.is_finished()) {
        return;
    }
    s.tls.stop();
    let Some(router) = s.tls.router.get().cloned() else { return };
    match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => {
            let port = l.local_addr().map(|a| a.port()).unwrap_or(addr.port());
            let handle = tokio::spawn(accept(l, router, s.tls.current.clone()));
            *s.tls.listener.lock().unwrap() = Some((addr, handle));
            *s.tls.status.lock().unwrap() = Listening { port: Some(port), error: None };
            tracing::info!("HTTPS on {}", SocketAddr::new(ip, port));
        }
        Err(e) => {
            let why = format!("cannot listen on port {}: {e}", addr.port());
            tracing::warn!("HTTPS: {why}");
            *s.tls.status.lock().unwrap() = Listening { port: None, error: Some(why) };
        }
    }
}

/// Accepte les connexions HTTPS et les sert comme les autres (WebSocket compris).
async fn accept(listener: tokio::net::TcpListener, router: Router, certs: Arc<ArcSwapOption<CertifiedKey>>) {
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("TLS versions")
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(Resolver(certs)));
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    loop {
        let Ok((tcp, peer)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        let (acceptor, router) = (acceptor.clone(), router.clone());
        tokio::spawn(async move {
            let Ok(Ok(tls)) = tokio::time::timeout(Duration::from_secs(10), acceptor.accept(tcp)).await else { return };
            let svc = hyper::service::service_fn(move |mut req: Request<hyper::body::Incoming>| {
                let router = router.clone();
                let ext = req.extensions_mut();
                ext.insert(ConnectInfo(peer));
                ext.insert(OverTls);
                req.headers_mut().insert("x-forwarded-proto", HeaderValue::from_static("https"));
                async move { router.oneshot(req.map(Body::new)).await }
            });
            let _ = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(tls), svc).with_upgrades().await;
        });
    }
}

/// En HTTP alors que le HTTPS écoute : vers la même adresse en HTTPS.
pub async fn redirect(State(s): State<Shared>, req: Request, next: Next) -> Response {
    let Some(port) = s.tls.listening().port else { return next.run(req).await };
    let path = req.uri().path();
    let exempt = path == "/api/health" || path == "/api/https/ca.crt";
    if req.extensions().get::<OverTls>().is_some() || exempt || !s.config.load().https.served() {
        return next.run(req).await;
    }
    let host = req.headers().get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
    if crate::guard::local_name(host) || host.is_empty() {
        return next.run(req).await;
    }
    let name = host.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map_or(host, |(h, _)| h);
    let port = if port == 443 { String::new() } else { format!(":{port}") };
    let pq = req.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let to = format!("https://{name}{port}{pq}");
    if matches!(*req.method(), axum::http::Method::GET | axum::http::Method::HEAD) {
        Redirect::permanent(&to).into_response()
    } else {
        // Une modification ne se rejoue pas d'elle-même ailleurs : refusée, avec l'adresse.
        (StatusCode::MISDIRECTED_REQUEST, format!("use {to}")).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_covered() {
        let n = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(covered("strata01.regie.fr", &n(&["strata01.regie.fr"])));
        assert!(covered("strata01.regie.fr", &n(&["*.regie.fr"])), "a wildcard of the level above");
        assert!(!covered("*.strata01.regie.fr", &n(&["*.regie.fr"])));
        assert!(covered("*.strata01.regie.fr", &n(&["*.Strata01.regie.fr"])));
        assert!(!covered("a.b.regie.fr", &n(&["*.regie.fr"])));
    }

    #[test]
    fn made_by_strata_then_renewed_for_a_new_name() {
        let dir = std::env::temp_dir().join(format!("strata-tls-{}", strata_core::new_id()));
        let (_, info) = ensure_strata(&dir, "strata01.regie.fr", "Régie A").unwrap();
        assert_eq!(info.names, ["strata01.regie.fr", "*.strata01.regie.fr"]);
        assert!(info.missing.is_empty());
        assert!(info.issuer.contains("Strata CA"));
        assert!(info.not_after - now() > 390 * 86400);
        let ca = std::fs::read_to_string(dir.join("strata-ca.crt")).unwrap();
        let first = std::fs::read_to_string(dir.join("strata.crt")).unwrap();
        // Rien ne change tant que le nom est le même ; un autre nom : un autre certificat, même autorité.
        ensure_strata(&dir, "strata01.regie.fr", "Régie A").unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("strata.crt")).unwrap(), first);
        let (_, info) = ensure_strata(&dir, "strata02.regie.fr", "Régie A").unwrap();
        assert_eq!(info.names[0], "strata02.regie.fr");
        assert_eq!(std::fs::read_to_string(dir.join("strata-ca.crt")).unwrap(), ca);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(dir.join("strata.key")).unwrap().permissions().mode() & 0o077, 0, "private keys stay private");
        }

        // L'import : une clé qui ne va pas, des noms non couverts, puis le bon.
        let chain = std::fs::read_to_string(dir.join("strata.crt")).unwrap();
        let key = std::fs::read_to_string(dir.join("strata.key")).unwrap();
        let other = std::fs::read_to_string(dir.join("strata-ca.key")).unwrap();
        assert_eq!(import(&dir, &chain, &other, "strata02.regie.fr").unwrap_err(), "the private key does not match the certificate");
        assert_eq!(import(&dir, &chain, &key, "strata03.regie.fr").unwrap_err(), "the certificate does not cover strata03.regie.fr, *.strata03.regie.fr");
        assert_eq!(import(&dir, "nothing", &key, "x").unwrap_err(), "there is no certificate in the file");
        assert_eq!(import(&dir, &chain, &key, "strata02.regie.fr").unwrap().origin, "company");
        assert!(load(&dir, "company", "strata02.regie.fr").is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
