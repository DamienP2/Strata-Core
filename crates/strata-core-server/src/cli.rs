//! La ligne de commande : comptes (`strata-core user …`) et applis (`strata-core app …`). Agit sur
//! le dossier du service installé s'il existe, sinon sur celui de l'utilisateur ; lancée avec sudo,
//! rend les fichiers au propriétaire du dossier. Repris de LiveCut.
use anyhow::{anyhow, bail, Context};
use clap::Subcommand;
use std::{
    io::{BufRead, IsTerminal},
    path::{Path, PathBuf},
};

use crate::{
    apps::Apps,
    auth::{self, Auth, NewUser},
};

#[derive(Subcommand)]
pub enum UserCmd {
    /// Ajoute un compte (demande le mot de passe deux fois).
    Add {
        name: String,
        /// Un utilisateur sans les réglages des serveurs (par défaut : administrateur).
        #[arg(long)]
        not_admin: bool,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Liste les comptes.
    List {
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Change le mot de passe d'un compte.
    Passwd {
        name: String,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Supprime un compte.
    Delete {
        name: String,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum AppCmd {
    /// Liste les applis installées.
    List {
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Active une appli installée.
    Activate {
        id: String,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Désactive une appli (elle reste installée).
    Deactivate {
        id: String,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
}

/// Demande un nouveau mot de passe deux fois (au terminal ; sur l'entrée standard, un par ligne).
fn ask_new_password() -> anyhow::Result<String> {
    let (a, b) = if std::io::stdin().is_terminal() {
        (rpassword::prompt_password("Password: ")?, rpassword::prompt_password("Repeat the password: ")?)
    } else {
        let mut lines = std::io::stdin().lock().lines();
        let mut next = || lines.next().transpose().map(|l| l.unwrap_or_default());
        (next()?, next()?)
    };
    if a != b {
        bail!("the passwords do not match");
    }
    auth::validate_password(&a).map_err(|e| anyhow!(e))?;
    Ok(a)
}

/// Lancée avec sudo : le fichier revient au propriétaire du dossier des données.
fn give_back(dir: &Path, file: &str) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid n'a aucune précondition.
        if unsafe { libc::geteuid() } != 0 {
            return;
        }
        if let Ok(m) = std::fs::metadata(dir) {
            if m.uid() != 0 {
                let _ = std::os::unix::fs::chown(dir.join(file), Some(m.uid()), Some(m.gid()));
            }
        }
    }
    #[cfg(not(unix))]
    let _ = (dir, file);
}

fn data_dir(given: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    let dir = crate::cli_data_dir(given);
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    Ok(dir)
}

pub fn user(cmd: UserCmd) -> anyhow::Result<()> {
    match cmd {
        UserCmd::Add { name, not_admin, data_dir: d } => {
            auth::validate_username(name.trim()).map_err(|e| anyhow!(e))?;
            let dir = data_dir(d)?;
            let auth = Auth::load(&dir);
            if auth.find_name(name.trim()).is_some() {
                bail!("this user name is already taken");
            }
            let password = ask_new_password()?;
            let u = auth.add(NewUser { username: name, display_name: String::new(), admin: !not_admin, password: Some(password) }).map_err(|e| anyhow!(e))?;
            give_back(&dir, "users.json");
            println!("user {} added ({}) in {}", u.username, if u.admin { "admin" } else { "user" }, dir.display());
        }
        UserCmd::List { data_dir: d } => {
            let dir = data_dir(d)?;
            let mut users = Auth::load(&dir).users();
            users.sort_by_key(|u| u.username.to_lowercase());
            if users.is_empty() {
                println!("no user yet in {}", dir.display());
                return Ok(());
            }
            let w = users.iter().map(|u| u.username.chars().count()).max().unwrap_or(4).max(4);
            println!("{:w$}  {:6}  {:24}  STATE", "USER", "ROLE", "DISPLAY NAME");
            for u in users {
                let mut state = Vec::new();
                if u.disabled {
                    state.push("disabled");
                }
                if !u.sso.is_empty() {
                    state.push("sso");
                }
                println!("{:w$}  {:6}  {:24}  {}", u.username, if u.admin { "admin" } else { "user" }, u.display_name, state.join(", "));
            }
        }
        UserCmd::Passwd { name, data_dir: d } => {
            let dir = data_dir(d)?;
            let auth = Auth::load(&dir);
            let u = auth.find_name(&name).ok_or_else(|| anyhow!("no such user: {name}"))?;
            let password = ask_new_password()?;
            auth.set_password(&u.id, &password).map_err(|e| anyhow!(e))?;
            give_back(&dir, "users.json");
            println!("password of {} changed", u.username);
        }
        UserCmd::Delete { name, data_dir: d } => {
            let dir = data_dir(d)?;
            let auth = Auth::load(&dir);
            let u = auth.find_name(&name).ok_or_else(|| anyhow!("no such user: {name}"))?;
            auth.delete(&u.id).map_err(|e| anyhow!(e))?;
            give_back(&dir, "users.json");
            println!("user {} deleted", u.username);
        }
    }
    Ok(())
}

pub fn app(cmd: AppCmd) -> anyhow::Result<()> {
    match cmd {
        AppCmd::List { data_dir: d } => {
            let dir = data_dir(d)?;
            let list = Apps::new(&dir).list();
            if list.is_empty() {
                println!("no app installed in {}", dir.join("apps").display());
                return Ok(());
            }
            println!("{:12}  {:12}  {:10}  {:8}  STATE", "ID", "NAME", "VERSION", "PORT");
            for a in list {
                let m = &a.manifest;
                let state = if !a.compatible {
                    format!("needs the Core {}", m.requires.core)
                } else if a.active {
                    "active".into()
                } else {
                    "inactive".into()
                };
                println!("{:12}  {:12}  {:10}  {:8}  {state}", m.id, m.name, m.version.to_string(), m.port);
            }
        }
        AppCmd::Activate { id, data_dir: d } => {
            let dir = data_dir(d)?;
            let a = Apps::new(&dir).set_active(&id, true).map_err(|e| anyhow!(e))?;
            give_back(&dir, "apps.json");
            println!("{} active", a.manifest.name);
        }
        AppCmd::Deactivate { id, data_dir: d } => {
            let dir = data_dir(d)?;
            let a = Apps::new(&dir).set_active(&id, false).map_err(|e| anyhow!(e))?;
            give_back(&dir, "apps.json");
            println!("{} inactive", a.manifest.name);
        }
    }
    Ok(())
}

#[derive(Subcommand)]
pub enum HttpsCmd {
    /// Repasse le Core en HTTP seul (si un certificat empêche d'ouvrir les réglages) ; à appliquer
    /// en redémarrant le service.
    Off {
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
}

pub fn https(cmd: HttpsCmd) -> anyhow::Result<()> {
    match cmd {
        HttpsCmd::Off { data_dir: d } => {
            let dir = data_dir(d)?;
            let mut cfg = crate::config::ServerConfig::load(&dir);
            cfg.https.mode = crate::config::HttpsMode::Off;
            cfg.save(&dir).context("cannot save config.json")?;
            give_back(&dir, "config.json");
            println!("HTTPS off in {} ; restart the service: sudo systemctl restart strata-core", dir.display());
        }
    }
    Ok(())
}
