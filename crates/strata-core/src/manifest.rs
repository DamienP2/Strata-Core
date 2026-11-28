//! La fiche d'une appli : `strata-app.toml`, livrée avec chaque release d'appli et posée dans
//! `<données du Core>/apps/<id>/` à l'installation.
//!
//! ```toml
//! id = "livecut"
//! name = "LiveCut"
//! version = "0.3.0"
//! icon = "scissors"   # une icône lucide
//! port = 8101         # le port interne de son service (jamais exposé : le Core relaie)
//!
//! [requires]
//! core = ">=0.1, <1"  # les versions du Core acceptées
//!
//! [run]               # facultatif : sans lui, l'appli est lancée à part (en développement)
//! command = "bin/strata-livecut"   # relatif au dossier de l'appli, ou absolu
//! args = ["serve"]
//!
//! [relay]             # facultatif
//! public = ["/output/"]  # chemins servis sans connexion (sorties pour un mélangeur…)
//! ```
//!
//! Le contrat complet (variables d'environnement, en-têtes d'identité) : docs/APPLIS.md.
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Pourquoi une fiche est refusée.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("cannot read {0}: {1}")]
    Read(String, std::io::Error),
    #[error("{0}: {1}")]
    Parse(String, toml::de::Error),
    #[error("{0}: {1}")]
    Invalid(String, String),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Requires {
    /// Les versions du Core acceptées (`>=0.1, <1`).
    pub core: VersionReq,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AppManifest {
    /// Minuscules, chiffres et tirets : c'est aussi son sous-domaine (`livecut.strata01…`).
    pub id: String,
    /// Le nom affiché (« LiveCut »).
    pub name: String,
    pub version: Version,
    /// Le nom d'une icône lucide (« scissors »).
    #[serde(default)]
    pub icon: String,
    /// Le port interne du service de l'appli.
    pub port: u16,
    pub requires: Requires,
    /// Comment le Core lance l'appli ; absent : elle est lancée à part, le Core ne fait que la relayer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<Run>,
    #[serde(default)]
    pub relay: Relay,
    /// Le chemin où l'appli dit si quelqu'un y a accès (`/strata/access`) ; absent : tout le monde.
    /// Le Core l'appelle avec les en-têtes `X-Strata-*` de la personne ; l'appli répond
    /// `{"access": true|false}` (ou 403). Le cockpit ne montre que les applis permises.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Run {
    /// Le programme : relatif au dossier de l'appli (`bin/strata-livecut`), ou absolu.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Relay {
    /// Les débuts de chemin servis sans connexion (`/output/`) : le Core les relaie sans identité.
    #[serde(default)]
    pub public: Vec<String>,
}

impl Relay {
    /// Ce chemin est-il public ?
    /// Un chemin ambigu (`..`, `.`, `//`, `\`, ou ces signes encodés en %) n'est jamais public :
    /// l'appli pourrait le ramener à une adresse privée (`/output/../api`).
    pub fn is_public(&self, path: &str) -> bool {
        let lower = path.to_ascii_lowercase();
        let ambiguous = path.contains("//")
            || path.contains('\\')
            || lower.contains("%2e")
            || lower.contains("%2f")
            || lower.contains("%5c")
            || path.split('/').any(|seg| seg == "." || seg == "..");
        if ambiguous {
            return false;
        }
        self.public.iter().any(|p| {
            // Sur une frontière de segment : `/health` couvre `/health` et `/health/…`, pas `/healthX`.
            let base = p.trim_end_matches('/');
            path == base || path.strip_prefix(base).is_some_and(|rest| rest.starts_with('/')) || (base.is_empty() && path.starts_with('/'))
        })
    }
}

impl AppManifest {
    /// Lit et vérifie une fiche.
    pub fn parse(text: &str, origin: &str) -> Result<Self, ManifestError> {
        let m: AppManifest = toml::from_str(text).map_err(|e| ManifestError::Parse(origin.to_string(), e))?;
        m.check().map_err(|e| ManifestError::Invalid(origin.to_string(), e))?;
        Ok(m)
    }

    /// Lit `strata-app.toml` dans un dossier d'appli.
    pub fn read(dir: &Path) -> Result<Self, ManifestError> {
        let path = dir.join("strata-app.toml");
        let origin = path.display().to_string();
        let text = std::fs::read_to_string(&path).map_err(|e| ManifestError::Read(origin.clone(), e))?;
        Self::parse(&text, &origin)
    }

    fn check(&self) -> Result<(), String> {
        valid_id(&self.id)?;
        if self.name.trim().is_empty() {
            return Err("the app needs a name".into());
        }
        if self.port < 1024 {
            return Err("the app's port must be 1024 or more".into());
        }
        if let Some(a) = &self.access {
            if !a.starts_with('/') || a.contains(['?', '#', ' ']) {
                return Err("access must be a path (/strata/access)".into());
            }
        }
        if let Some(run) = &self.run {
            if run.command.trim().is_empty() {
                return Err("[run] needs a command".into());
            }
        }
        if let Some(p) = self.relay.public.iter().find(|p| !p.starts_with('/') || p.len() < 2 || p.contains("..")) {
            return Err(format!("[relay] public paths start with / and are not / alone: {p:?}"));
        }
        Ok(())
    }

    /// Cette appli accepte-t-elle ce Core ?
    pub fn accepts(&self, core: &Version) -> bool {
        accepts(&self.requires.core, core)
    }
}

/// `core` est-il dans la plage ? Une préversion du Core est acceptée comme la version qu'elle précède
/// (`0.2.0-rc.1` passe `>=0.2`), ce que semver refuse par défaut.
pub fn accepts(req: &VersionReq, core: &Version) -> bool {
    if req.matches(core) {
        return true;
    }
    if !core.pre.is_empty() {
        let release = Version::new(core.major, core.minor, core.patch);
        return req.matches(&release);
    }
    false
}

/// Un identifiant d'appli : 2 à 32 minuscules, chiffres ou tirets, commençant par une lettre.
pub fn valid_id(id: &str) -> Result<(), String> {
    let ok = (2..=32).contains(&id.len())
        && id.starts_with(|c: char| c.is_ascii_lowercase())
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !id.ends_with('-');
    if ok {
        Ok(())
    } else {
        Err(format!("invalid app id: {id:?} (2 to 32 lower-case letters, digits or dashes)"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIVECUT: &str = r#"
id = "livecut"
name = "LiveCut"
version = "0.3.0"
icon = "scissors"
port = 8101

[requires]
core = ">=0.1, <1"
"#;

    #[test]
    fn reads_a_manifest() {
        let m = AppManifest::parse(LIVECUT, "test").unwrap();
        assert_eq!(m.id, "livecut");
        assert_eq!(m.version, Version::new(0, 3, 0));
        assert!(m.accepts(&Version::new(0, 1, 0)));
        assert!(m.accepts(&Version::new(0, 9, 4)));
        assert!(!m.accepts(&Version::new(1, 0, 0)));
        assert!(!m.accepts(&Version::new(0, 0, 9)));
        assert!(m.accepts(&Version::parse("0.2.0-rc.1").unwrap()), "a pre-release counts as its release");
        assert!(m.run.is_none());
        assert!(!m.relay.is_public("/output/a"));
    }

    #[test]
    fn run_and_public_paths() {
        let text = format!("{LIVECUT}\n[run]\ncommand = \"bin/livecut\"\nargs = [\"serve\"]\n\n[relay]\npublic = [\"/output/\", \"/health\"]\n");
        let m = AppManifest::parse(&text, "t").unwrap();
        assert_eq!(m.run.as_ref().unwrap().args, ["serve"]);
        assert!(m.relay.is_public("/output/pgm.html"));
        assert!(m.relay.is_public("/output"));
        assert!(m.relay.is_public("/health"));
        assert!(!m.relay.is_public("/api/state"));
        // Ni détour, ni préfixe sans frontière.
        assert!(!m.relay.is_public("/output/../api/state"));
        assert!(!m.relay.is_public("/output/%2e%2e/api/state"));
        assert!(!m.relay.is_public("/output//x"));
        assert!(!m.relay.is_public("/healthX"));
        assert!(m.relay.is_public("/health/live"));
        assert!(AppManifest::parse(&text.replace("\"/health\"", "\"/\""), "t").is_err(), "/ alone would open everything");
        assert!(AppManifest::parse(&text.replace("bin/livecut", " "), "t").is_err());
    }

    #[test]
    fn refuses_bad_manifests() {
        assert!(AppManifest::parse("id = \"x\"", "t").is_err(), "missing fields");
        assert!(AppManifest::parse(&LIVECUT.replace("livecut", "Live Cut"), "t").is_err(), "bad id");
        assert!(AppManifest::parse(&LIVECUT.replace("8101", "80"), "t").is_err(), "low port");
        assert!(AppManifest::parse(&LIVECUT.replace(">=0.1, <1", "whatever"), "t").is_err(), "bad range");
        assert!(valid_id("multiview").is_ok());
        assert!(valid_id("a").is_err());
        assert!(valid_id("1app").is_err());
        assert!(valid_id("app-").is_err());
    }
}
