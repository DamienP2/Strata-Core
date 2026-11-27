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
    /// `https` ou `http` : le début des adresses des applis.
    pub scheme: String,
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
            scheme: "https".into(),
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
            if let Err(e) = strata_core::files::write_json(&path, &c, false) {
                tracing::warn!("cannot write {}: {e}", path.display());
            }
        }
        c
    }

    fn normalized(mut self) -> Self {
        self.name = self.name.trim().to_string();
        self.dns_name = self.dns_name.trim().trim_end_matches('.').to_lowercase();
        if self.name.is_empty() {
            self.name = hostname();
        }
        if self.dns_name.is_empty() {
            self.dns_name = format!("{}.local", hostname());
        }
        if self.scheme != "http" {
            self.scheme = "https".into();
        }
        self
    }

    /// L'adresse d'une appli : `https://livecut.strata01.regie.exemple.fr/`.
    pub fn app_url(&self, id: &str) -> String {
        format!("{}://{id}.{}/", self.scheme, self.dns_name)
    }

    /// Le domaine du cookie de session quand la requête arrive par le nom du serveur ou un de ses
    /// sous-domaines : la session vaut alors pour toutes les applis. Par adresse IP ou localhost : aucun.
    pub fn cookie_domain(&self, host: Option<&str>) -> Option<String> {
        let host = host?.split(':').next()?.to_lowercase();
        let base = &self.dns_name;
        (host == *base || host.ends_with(&format!(".{base}"))).then(|| base.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_and_cookie_domain() {
        let c = ServerConfig { dns_name: "strata01.regie.exemple.fr".into(), ..ServerConfig::default() };
        assert_eq!(c.app_url("livecut"), "https://livecut.strata01.regie.exemple.fr/");
        assert_eq!(c.cookie_domain(Some("strata01.regie.exemple.fr")).as_deref(), Some("strata01.regie.exemple.fr"));
        assert_eq!(c.cookie_domain(Some("livecut.strata01.regie.exemple.fr:443")).as_deref(), Some("strata01.regie.exemple.fr"));
        assert_eq!(c.cookie_domain(Some("10.0.0.20:8080")), None);
        assert_eq!(c.cookie_domain(Some("evilstrata01.regie.exemple.fr")), None);
        assert_eq!(c.cookie_domain(None), None);
    }

    #[test]
    fn incomplete_file() {
        let c: ServerConfig = serde_json::from_str(r#"{"name":"  Régie A ","dnsName":"Strata01.Local.","scheme":"ftp"}"#).unwrap();
        let c = c.normalized();
        assert_eq!(c.name, "Régie A");
        assert_eq!(c.dns_name, "strata01.local");
        assert_eq!(c.scheme, "https");
        assert_eq!(c.timezone, "Europe/Paris");
    }
}
