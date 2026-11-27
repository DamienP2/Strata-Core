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
        Some(Cmd::Run(r)) => run(r),
        None => run(cli.run),
    }
}

fn run(a: RunArgs) -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=warn".into())).init();
    let data_dir = a.data_dir.unwrap_or_else(strata_core_server::user_data_dir);
    // Retenu avant tout : une mise à jour remplace le fichier, et son chemin ne se relirait plus.
    let exe = std::env::current_exe()?;
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
