//! Ce que le Core et les applis Strata partagent côté serveur.
//!
//! - [`files`] : les fichiers de données (JSON écrit de façon atomique, fichiers privés en 0600) ;
//! - [`manifest`] : la fiche d'une appli (`strata-app.toml`) et la plage de versions du Core qu'elle accepte ;
//! - [`VERSION`] : la version de ce Core.

pub mod files;
pub mod manifest;

/// La version du Core (celle du workspace, reprise de l'étiquette de release).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Un identifiant court et aléatoire (12 caractères hexadécimaux).
pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
}
