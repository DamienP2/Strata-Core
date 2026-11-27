//! La connexion par l'annuaire de l'entreprise : OpenID Connect (Microsoft Entra, Okta, Keycloak,
//! Google…). Repris de LiveCut, adapté au Core : les groupes de l'annuaire donnent les groupes du Core
//! et le rôle d'administrateur des serveurs.
//!
//! Ici, les réglages ; la connexion elle-même (découverte, PKCE, vérification du jeton) suit.
use serde::{Deserialize, Serialize};

use crate::groups::clean_directory_group;

/// Un secret tel que l'API le renvoie.
pub const MASK: &str = "••••••••";
pub const PROVIDERS: [&str; 5] = ["entra", "okta", "keycloak", "google", "other"];

/// Qui peut encore se connecter avec un mot de passe local quand le SSO est actif.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LocalAccounts {
    /// Personne (sauf depuis le serveur lui-même, pour réparer).
    #[serde(rename = "none")]
    Off,
    /// Les administrateurs seulement (une porte de secours).
    #[default]
    #[serde(rename = "adminOnly")]
    AdminOnly,
    #[serde(rename = "all")]
    All,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct SsoConfig {
    pub enabled: bool,
    pub provider: String,
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    /// La revendication du jeton qui porte les groupes.
    pub groups_claim: String,
    /// Les groupes de l'annuaire qui font un administrateur des serveurs.
    pub admin_groups: Vec<String>,
    /// Accepter aussi les comptes de l'annuaire qui ne sont dans aucun groupe connu.
    pub allow_any: bool,
    pub local_accounts: LocalAccounts,
    /// Les domaines d'e-mail autorisés (vide : tous ; Google en exige, sinon tout compte Google entre).
    pub domains: Vec<String>,
}

impl Default for SsoConfig {
    fn default() -> Self {
        SsoConfig {
            enabled: false,
            provider: "entra".into(),
            issuer: String::new(),
            client_id: String::new(),
            client_secret: String::new(),
            groups_claim: "groups".into(),
            admin_groups: Vec::new(),
            allow_any: false,
            local_accounts: LocalAccounts::default(),
            domains: Vec::new(),
        }
    }
}

impl SsoConfig {
    pub fn normalize(&mut self) {
        self.provider = self.provider.trim().to_lowercase();
        if !PROVIDERS.contains(&self.provider.as_str()) {
            self.provider = "other".into();
        }
        self.issuer = self.issuer.trim().trim_end_matches('/').to_string();
        self.client_id = self.client_id.trim().to_string();
        self.client_secret = self.client_secret.trim().to_string();
        self.groups_claim = self.groups_claim.trim().to_string();
        if self.groups_claim.is_empty() {
            self.groups_claim = "groups".into();
        }
        self.admin_groups = self.admin_groups.iter().map(|g| g.trim().to_string()).filter(|g| !g.is_empty()).collect();
        self.domains = self.domains.iter().map(|d| d.trim().trim_start_matches('@').to_lowercase()).filter(|d| !d.is_empty()).collect();
    }

    /// Actif, avec un émetteur et un identifiant client.
    pub fn active(&self) -> bool {
        self.enabled && !self.issuer.is_empty() && !self.client_id.is_empty()
    }

    /// Le nom sur le bouton de connexion.
    pub fn button_name(&self) -> &'static str {
        match self.provider.as_str() {
            "entra" => "Microsoft",
            "okta" => "Okta",
            "keycloak" => "Keycloak",
            "google" => "Google",
            _ => "SSO",
        }
    }

    /// Un compte (administrateur ou non) peut-il se connecter avec un mot de passe ?
    pub fn local_allowed(&self, admin: bool) -> bool {
        if !self.active() {
            return true;
        }
        match self.local_accounts {
            LocalAccounts::Off => false,
            LocalAccounts::AdminOnly => admin,
            LocalAccounts::All => true,
        }
    }

    /// Ce que la page de connexion doit savoir : qui peut utiliser un compte local en ce moment.
    pub fn local_accounts_now(&self) -> LocalAccounts {
        if self.active() {
            self.local_accounts
        } else {
            LocalAccounts::All
        }
    }

    /// Ces groupes de l'annuaire font-ils un administrateur des serveurs ?
    pub fn is_admin(&self, directory_groups: &[String]) -> bool {
        let mine: Vec<String> = directory_groups.iter().map(|g| clean_directory_group(g)).collect();
        self.admin_groups.iter().any(|a| mine.contains(&clean_directory_group(a)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn config() {
        let mut c: SsoConfig = serde_json::from_value(json!({"enabled": true, "provider": "Okta", "issuer": " https://id.example.com/ ", "clientId": "abc", "adminGroups": [" ", "/IT-Admins"], "domains": ["@Example.org"]})).unwrap();
        c.normalize();
        assert_eq!(c.issuer, "https://id.example.com");
        assert_eq!(c.admin_groups, vec!["/IT-Admins".to_string()]);
        assert_eq!(c.domains, vec!["example.org".to_string()]);
        assert!(c.active());
        assert_eq!(c.button_name(), "Okta");
        assert_eq!(c.local_accounts, LocalAccounts::AdminOnly);
        assert!(c.local_allowed(true) && !c.local_allowed(false));
        assert!(c.is_admin(&["it-admins".into()]));
        assert!(!c.is_admin(&["other".into()]));
        assert_eq!(serde_json::to_value(LocalAccounts::Off).unwrap(), json!("none"));
        assert!(SsoConfig::default().local_allowed(false), "off: everyone");
        assert_eq!(SsoConfig::default().local_accounts_now(), LocalAccounts::All);
    }
}
