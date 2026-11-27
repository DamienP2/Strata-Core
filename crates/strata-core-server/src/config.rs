//! Les réglages du serveur (`config.json`). Chaque champ a une valeur par défaut : un fichier ancien,
//! incomplet ou absent donne un serveur qui démarre.
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, path::Path};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ServerConfig {
    /// Le nom d'usage du serveur, vu par les utilisateurs (« Régie A »).
    pub name: String,
    /// Son nom DNS (« strata01.regie.exemple.fr ») : le cockpit y répond, chaque appli à son
    /// sous-domaine (`livecut.strata01.regie.exemple.fr`).
    pub dns_name: String,
    /// L'adresse d'écoute (sinon `--bind`, sinon 0.0.0.0:8080).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bind: Option<SocketAddr>,
    /// Les origines autorisées à envoyer des modifications avec le cookie (CSRF).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub trusted_origins: Vec<String>,
    pub timezone: String,
    /// Le HTTPS : aucun, fait par Strata, de l'entreprise, ou fait par un relais devant le Core.
    pub https: HttpsConfig,
    /// La connexion par l'annuaire de l'entreprise (OpenID Connect).
    pub sso: crate::sso::SsoConfig,
}

/// D'où vient le HTTPS.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum HttpsMode {
    /// Aucun : HTTP seulement.
    #[default]
    Off,
    /// Un certificat fait par Strata, signé par l'autorité de ce serveur (à installer une fois sur
    /// chaque poste).
    Strata,
    /// Le certificat de l'entreprise, importé.
    Company,
    /// Un relais devant le Core fait le HTTPS ; le Core reste en HTTP, ses adresses en `https://`.
    Proxy,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct HttpsConfig {
    pub mode: HttpsMode,
    /// Le port HTTPS du Core (Strata, entreprise).
    pub port: u16,
}

impl Default for HttpsConfig {
    fn default() -> Self {
        HttpsConfig { mode: HttpsMode::Off, port: 443 }
    }
}

impl HttpsConfig {
    /// Le Core écoute-t-il lui-même en HTTPS ?
    pub fn served(&self) -> bool {
        matches!(self.mode, HttpsMode::Strata | HttpsMode::Company)
    }
}

/// Le nom de la machine (« strata01 »).
pub fn hostname() -> String {
    gethostname::gethostname().to_string_lossy().split('.').next().unwrap_or("strata").to_lowercase()
}

impl Default for ServerConfig {
    fn default() -> Self {
        let host = hostname();
        ServerConfig {
            name: host.clone(),
            dns_name: format!("{host}.local"),
            bind: None,
            trusted_origins: Vec::new(),
            timezone: "Europe/Paris".into(),
            https: HttpsConfig::default(),
            sso: crate::sso::SsoConfig::default(),
        }
    }
}

impl ServerConfig {
    /// Lit `config.json` ; absent, l'écrit avec les valeurs par défaut.
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("config.json");
        if let Some(c) = strata_core::files::read_json::<ServerConfig>(&path) {
            return c.normalized();
        }
        let c = ServerConfig::default();
        if !path.exists() {
            if let Err(e) = c.save(data_dir) {
                tracing::warn!("cannot write {}: {e}", path.display());
            }
        }
        c
    }

    /// Enregistre `config.json` (privé : il garde le secret du SSO).
    pub fn save(&self, data_dir: &Path) -> std::io::Result<()> {
        strata_core::files::write_json(&data_dir.join("config.json"), self, true)
    }

    /// `https` ou `http` : le début des adresses du cockpit et des applis.
    pub fn scheme(&self) -> &'static str {
        if self.https.mode == HttpsMode::Off {
            "http"
        } else {
            "https"
        }
    }

    /// L'adresse de retour à donner au fournisseur d'identité.
    pub fn sso_redirect_uri(&self) -> String {
        format!("{}://{}/api/auth/sso/callback", self.scheme(), self.dns_name)
    }

    pub(crate) fn normalized(mut self) -> Self {
        self.sso.normalize();
        self.name = self.name.trim().to_string();
        self.dns_name = self.dns_name.trim().trim_end_matches('.').to_lowercase();
        if self.name.is_empty() {
            self.name = hostname();
        }
        if self.dns_name.is_empty() {
            self.dns_name = format!("{}.local", hostname());
        }
        if self.https.port == 0 {
            self.https.port = 443;
        }
        self
    }

    /// L'adresse d'une appli : `https://livecut.strata01.regie.exemple.fr/`.
    pub fn app_url(&self, id: &str) -> String {
        format!("{}://{id}.{}/", self.scheme(), self.dns_name)
    }

    /// L'adresse d'une appli vue depuis une requête arrivée par `host` : avec son port s'il en a un
    /// (`http://livecut.strata01.local:8080/`), pour un Core qui n'écoute pas sur 80 ou 443.
    pub fn app_url_at(&self, id: &str, host: Option<&str>) -> String {
        match host_port(host) {
            Some(port) => format!("{}://{id}.{}:{port}/", self.scheme(), self.dns_name),
            None => self.app_url(id),
        }
    }

    /// L'adresse du cockpit vue depuis une requête arrivée par `host`.
    pub fn core_url_at(&self, host: Option<&str>) -> String {
        match host_port(host) {
            Some(port) => format!("{}://{}:{port}/", self.scheme(), self.dns_name),
            None => format!("{}://{}/", self.scheme(), self.dns_name),
        }
    }

    /// Où revenir après la connexion : un chemin du cockpit (`/reglages`), ou l'adresse d'une appli de
    /// ce serveur (`https://livecut.strata01.local/…`) ; autre chose : le cockpit.
    pub fn safe_return(&self, next: Option<&str>) -> String {
        let Some(n) = next else { return "/".into() };
        if n.starts_with('/') {
            return crate::sso::safe_next(Some(n));
        }
        let clean = !n.contains('\\') && !n.chars().any(|c| c.is_control() || c.is_whitespace());
        let host = n.strip_prefix("https://").or_else(|| n.strip_prefix("http://")).map(|r| r.split(['/', '?', '#']).next().unwrap_or(""));
        let ok = clean && host.is_some_and(|h| !h.contains('@') && crate::relay::app_of_host(&self.dns_name, h).is_some());
        if ok {
            n.to_string()
        } else {
            "/".into()
        }
    }

    /// Le domaine du cookie de session quand la requête arrive par le nom du serveur ou un de ses
    /// sous-domaines : la session vaut alors pour toutes les applis. Par adresse IP ou localhost : aucun.
    pub fn cookie_domain(&self, host: Option<&str>) -> Option<String> {
        let host = host?.split(':').next()?.to_lowercase();
        let base = &self.dns_name;
        (host == *base || host.ends_with(&format!(".{base}"))).then(|| base.clone())
    }
}

/// Le port d'un `Host` (`strata01.local:8080` → 8080), sauf 80 et 443.
fn host_port(host: Option<&str>) -> Option<u16> {
    let host = host?;
    if host.ends_with(']') {
        return None;
    }
    let port: u16 = host.rsplit_once(':')?.1.parse().ok()?;
    (port != 80 && port != 443).then_some(port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_and_cookie_domain() {
        let c =
            ServerConfig { dns_name: "strata01.regie.exemple.fr".into(), https: HttpsConfig { mode: HttpsMode::Proxy, port: 443 }, ..ServerConfig::default() };
        assert_eq!(c.app_url("livecut"), "https://livecut.strata01.regie.exemple.fr/");
        assert_eq!(c.cookie_domain(Some("strata01.regie.exemple.fr")).as_deref(), Some("strata01.regie.exemple.fr"));
        assert_eq!(c.cookie_domain(Some("livecut.strata01.regie.exemple.fr:443")).as_deref(), Some("strata01.regie.exemple.fr"));
        assert_eq!(c.cookie_domain(Some("10.0.0.20:8080")), None);
        assert_eq!(c.cookie_domain(Some("evilstrata01.regie.exemple.fr")), None);
        assert_eq!(c.cookie_domain(None), None);
        assert_eq!(c.app_url_at("livecut", Some("10.0.0.20:8080")), "https://livecut.strata01.regie.exemple.fr:8080/");
        assert_eq!(c.app_url_at("livecut", Some("strata01.regie.exemple.fr:443")), "https://livecut.strata01.regie.exemple.fr/");
        assert_eq!(c.app_url_at("livecut", Some("[::1]")), "https://livecut.strata01.regie.exemple.fr/");
        assert_eq!(c.core_url_at(Some("livecut.strata01.regie.exemple.fr:8090")), "https://strata01.regie.exemple.fr:8090/");
        let app = "https://livecut.strata01.regie.exemple.fr:8090/rushes?x=1";
        assert_eq!(c.safe_return(Some(app)), app);
        assert_eq!(c.safe_return(Some("/reglages")), "/reglages");
        assert_eq!(c.safe_return(Some("https://evil.com/")), "/");
        assert_eq!(c.safe_return(Some("https://livecut.strata01.regie.exemple.fr.evil.com/")), "/");
        assert_eq!(c.safe_return(Some("https://user@livecut.strata01.regie.exemple.fr/")), "/");
        assert_eq!(c.safe_return(Some("javascript:alert(1)")), "/");
        assert_eq!(c.safe_return(None), "/");
    }

    #[test]
    fn incomplete_file() {
        let c: ServerConfig = serde_json::from_str(r#"{"name":"  Régie A ","dnsName":"Strata01.Local.","https":{"mode":"strata"}}"#).unwrap();
        let c = c.normalized();
        assert_eq!(c.name, "Régie A");
        assert_eq!(c.dns_name, "strata01.local");
        assert_eq!((c.scheme(), c.https.port), ("https", 443));
        assert_eq!(ServerConfig::default().scheme(), "http");
        assert_eq!(c.timezone, "Europe/Paris");
    }
}
