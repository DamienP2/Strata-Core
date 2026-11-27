//! `strata-core` : le service (par défaut), et les commandes `user` et `app`.
use clap::{Args, Parser, Subcommand};
use std::{net::SocketAddr, path::PathBuf};
use strata_core_server::cli;

#[derive(Parser)]
#[command(name = "strata-core", version, about = "Strata Core: the cockpit and the base of a Strata server", args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    #[command(flatten)]
    run: RunArgs,
}

#[derive(Subcommand)]
enum Cmd {
    /// Lance le service (par défaut).
    Run(RunArgs),
    /// Les comptes.
    #[command(subcommand)]
    User(cli::UserCmd),
    /// Les applis installées.
    #[command(subcommand)]
    App(cli::AppCmd),
    /// Le HTTPS du Core.
    #[command(subcommand)]
    Https(cli::HttpsCmd),
    /// Le nom du serveur, son nom DNS, son fuseau.
    Config(cli::ConfigArgs),
    /// Le service ptp4l de Strata (posé par l'installateur).
    Ptp4lUnit,
    /// Le service strata-media (posé par l'installateur).
    MediaUnit,
    /// Hugepages et ports DPDK pour le ST 2110 (lancé en root par strata-media.service).
    MediaSetup {
        /// Le compte du service, à qui donner les ports DPDK.
        #[arg(long, default_value = "strata")]
        user: String,
    },
}

#[derive(Args, Default)]
struct RunArgs {
    /// L'adresse d'écoute (sinon celle des réglages, sinon 0.0.0.0:8080).
    #[arg(long, env = "STRATA_BIND")]
    bind: Option<SocketAddr>,
    /// Le dossier des données.
    #[arg(long, env = "STRATA_DATA_DIR")]
    data_dir: Option<PathBuf>,
    /// Sert l'interface depuis ce dossier au lieu de celle embarquée (développement).
    #[arg(long, env = "STRATA_WEB_DIR")]
    web_dir: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Some(Cmd::User(c)) => cli::user(c),
        Some(Cmd::App(c)) => cli::app(c),
        Some(Cmd::Https(c)) => cli::https(c),
        Some(Cmd::Config(c)) => cli::config(c),
        Some(Cmd::Ptp4lUnit) => {
            print!("{}", strata_core_server::hardware::clock::PTP4L_UNIT);
            Ok(())
        }
        Some(Cmd::MediaUnit) => {
            print!("{}", strata_core_server::hardware::media::UNIT);
            Ok(())
        }
        Some(Cmd::MediaSetup { user }) => strata_core_server::hardware::media::setup(&user).map_err(|e| anyhow::anyhow!(e)),
        Some(Cmd::Run(r)) => run(r),
        None => run(cli.run),
    }
}

fn run(a: RunArgs) -> anyhow::Result<()> {
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
    // La console, et l'anneau qui tient le journal de Santé.
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=warn".into()))
        .with(tracing_subscriber::fmt::layer())
        .with(strata_core_server::logs::Ring)
        .init();
    let data_dir = a.data_dir.unwrap_or_else(strata_core_server::user_data_dir);
    // Le journal sur disque, les plantages.
    let _ = std::fs::create_dir_all(&data_dir);
    strata_core_server::health::start(&data_dir);
    // Les réglages réseau de NDI que partagent le Core et les applis (Matériel › NDI) ; avant
    // tout fil, la bibliothèque NDI le lit en se chargeant.
    if std::env::var_os("NDI_CONFIG_DIR").is_none() {
        std::env::set_var("NDI_CONFIG_DIR", strata_core_server::hardware::ndi::config_dir(&data_dir));
    }
    // Retenu avant tout : une mise à jour remplace le fichier, et son chemin ne se relirait plus.
    let exe = std::env::current_exe()?;
    // Une mise à jour qui ne tient pas : l'ancien Core est remis et relancé.
    if strata_core_server::updates::guard_start(&exe) {
        return restart(&exe);
    }
    // Une minute de marche confirme une mise à jour.
    {
        let exe = exe.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(60));
            strata_core_server::updates::confirm_start(&exe);
        });
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let ending = rt.block_on(strata_core_server::serve(a.bind, data_dir, a.web_dir))?;
    rt.shutdown_timeout(std::time::Duration::from_secs(2));
    if ending == strata_core_server::Ending::Restart {
        restart(&exe)?;
    }
    Ok(())
}

/// Redémarre le Core : le même programme, avec les mêmes arguments, à la place de celui-ci
/// (sous systemd, le service garde son numéro de processus).
#[cfg(unix)]
fn restart(exe: &std::path::Path) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;
    tracing::info!("restarting");
    Err(std::process::Command::new(exe).args(std::env::args_os().skip(1)).exec().into())
}

#[cfg(not(unix))]
fn restart(_exe: &std::path::Path) -> anyhow::Result<()> {
    anyhow::bail!("restart is only supported on Linux")
}
