//! `strata-install` : installe un serveur Strata sur Ubuntu 24.04 (brief § 5.7).
//!
//! Dans un terminal : un écran plein terminal, piloté au clavier. Sans terminal, ou avec `--yes` :
//! les mêmes choix en options. `--root <dossier>` : essai à blanc, tout sous ce dossier.
mod checks;
mod ndi;
mod paths;
mod release;
mod steps;
mod ui;

use std::{io::IsTerminal, path::PathBuf};

use clap::Parser;
use steps::{Action, Event, Plan};

#[derive(Parser, Debug, Clone, Default)]
#[command(name = "strata-install", version, about = "Installe un serveur Strata (Ubuntu 24.04)")]
pub struct Args {
    /// Sans écran : tout vient des options.
    #[arg(long)]
    yes: bool,
    /// Met à jour le Core (et les applis données) d'un serveur déjà installé.
    #[arg(long, conflicts_with_all = ["repair", "uninstall"])]
    update: bool,
    /// Remet en place les fichiers du système et les droits, puis relance.
    #[arg(long, conflicts_with = "uninstall")]
    repair: bool,
    /// Désinstalle (les données sont gardées, sauf --purge).
    #[arg(long)]
    uninstall: bool,
    /// Avec --uninstall : efface aussi les données.
    #[arg(long)]
    purge: bool,
    /// Le nom d'usage du serveur (« Régie A »).
    #[arg(long)]
    name: Option<String>,
    /// Son nom DNS (« strata01.regie.exemple.fr »).
    #[arg(long)]
    dns_name: Option<String>,
    #[arg(long, default_value = "Europe/Paris")]
    timezone: String,
    /// Les applis à installer et activer : dépôts GitHub (owner/repo) ou archives, séparés par des virgules.
    #[arg(long, value_delimiter = ',')]
    apps: Vec<String>,
    /// Le compte administrateur (mot de passe : STRATA_ADMIN_PASSWORD, sinon demandé).
    #[arg(long)]
    admin: Option<String>,
    /// Le pilote Blackmagic : son archive Desktop Video (ou « auto » : celle copiée sur le serveur).
    #[arg(long)]
    desktop_video: Option<String>,
    /// Installe la bibliothèque NDI (avec --accept-ndi-license).
    #[arg(long)]
    ndi: bool,
    /// Accepte la licence du SDK NDI (https://ndi.video/legal/).
    #[arg(long)]
    accept_ndi_license: bool,
    /// Installe ce qu'il faut aux GPU présents : le pilote NVIDIA, VA-API pour Intel et AMD.
    #[arg(long)]
    gpu_drivers: bool,
    /// N'ouvre pas les ports du cockpit dans le pare-feu.
    #[arg(long)]
    no_firewall: bool,
    /// L'archive du Core (serveur sans internet).
    #[arg(long)]
    package: Option<PathBuf>,
    /// La version du Core (sinon la dernière).
    #[arg(long)]
    version_core: Option<String>,
    /// Essai à blanc : tout sous ce dossier, les commandes du système seulement écrites au journal.
    #[arg(long)]
    root: Option<PathBuf>,
}

fn fail(msg: &str) -> ! {
    eprintln!("✗ {msg}");
    std::process::exit(1);
}

/// Le plan venu des options.
fn plan_of(a: &Args) -> Plan {
    let action = if a.uninstall {
        Action::Uninstall { purge: a.purge }
    } else if a.update {
        Action::Update
    } else if a.repair {
        Action::Repair
    } else {
        Action::Install
    };
    let mut p = Plan {
        action,
        timezone: a.timezone.clone(),
        apps: a.apps.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        firewall: !a.no_firewall,
        package: a.package.clone(),
        version: a.version_core.clone(),
        ..Plan::default()
    };
    if p.action == Action::Install {
        let dns = a.dns_name.clone().unwrap_or_default().trim().trim_end_matches('.').to_lowercase();
        if !checks::valid_dns(&dns) {
            fail("give the DNS name of the server: --dns-name strata01.regie.exemple.fr");
        }
        p.name = a.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| dns.split('.').next().unwrap_or_default().to_string());
        p.dns_name = dns;
        if let Some(name) = &a.admin {
            let pw = std::env::var("STRATA_ADMIN_PASSWORD").ok().filter(|p| !p.is_empty()).unwrap_or_else(|| {
                eprint!("Password of {name}: ");
                let mut l = String::new();
                let _ = std::io::stdin().read_line(&mut l);
                l.trim_end().to_string()
            });
            if pw.chars().count() < 8 {
                fail("the administrator password has 8 characters at least");
            }
            p.admin = Some((name.clone(), pw));
        }
    }
    if let Some(dv) = &a.desktop_video {
        p.desktop_video = match dv.as_str() {
            "auto" => checks::find_desktop_video(),
            "skip" | "" => None,
            path => Some(PathBuf::from(path)),
        };
        if dv == "auto" && p.desktop_video.is_none() {
            eprintln!("! no Desktop Video archive on this server: the Blackmagic driver is skipped");
        }
    }
    if a.gpu_drivers {
        (p.nvidia, p.vaapi) = checks::gpu_needs(&checks::gpus());
    }
    if (p.nvidia || p.desktop_video.is_some()) && checks::secure_boot() {
        eprintln!("! Secure Boot is on: at the next boot, enrol the MOK key, or the Blackmagic / NVIDIA modules will not load");
    }
    if a.ndi {
        if !a.accept_ndi_license {
            fail("NDI: read its license (https://ndi.video/legal/), then add --accept-ndi-license");
        }
        let dir = std::env::temp_dir().join("strata-install-ndi");
        match ndi::fetch(&dir) {
            Ok(sdk) => p.ndi = Some(sdk.lib),
            Err(e) => eprintln!("! NDI skipped: {e}"),
        }
    }
    p
}

/// Sans écran : les vérifications, puis les étapes, une ligne chacune.
fn headless(a: &Args) {
    let root = a.root.clone().unwrap_or_else(|| PathBuf::from("/"));
    let dry = root != std::path::Path::new("/");
    if !dry && a.update && checks::installed_core(&root).is_none() {
        fail("Strata Core is not installed here");
    }
    if !dry && !a.uninstall {
        let mut blocked = false;
        for c in checks::system(a.package.is_some()) {
            let mark = match c.level {
                checks::Level::Ok => "✓",
                checks::Level::Warn => "!",
                checks::Level::Bad => {
                    blocked = true;
                    "✗"
                }
            };
            println!("{mark} {:10} {}", c.label, c.detail);
        }
        if blocked {
            fail("this server cannot take Strata (see above)");
        }
    }
    let plan = plan_of(a);
    let ctx = steps::Ctx::new(&root);
    let list = steps::steps(&plan);
    let labels: Vec<String> = list.iter().map(|s| s.label.clone()).collect();
    let ok = steps::run(&ctx, &list, &|e| match e {
        Event::Start(i) => println!("… {}", labels[i]),
        Event::Progress(_, _) => {}
        Event::Done(i, Ok(m)) => println!("✓ {} : {m}", labels[i]),
        Event::Done(i, Err(e)) => println!("✗ {} : {e}", labels[i]),
        Event::End(_) => {}
    });
    println!("journal : {}", paths::under(&root, paths::LOG).display());
    if !ok {
        std::process::exit(1);
    }
    if plan.action == Action::Install {
        println!("\nStrata est installé. Le cockpit : http://{}/", plan.dns_name);
    }
}

fn main() {
    let a = Args::parse();
    if a.yes || !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        headless(&a);
    } else if let Err(e) = ui::run(a) {
        fail(&e.to_string());
    }
}
