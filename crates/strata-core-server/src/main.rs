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
        Some(Cmd::Run(r)) => run(r),
        None => run(cli.run),
    }
}

fn run(a: RunArgs) -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=warn".into())).init();
    let data_dir = a.data_dir.unwrap_or_else(strata_core_server::user_data_dir);
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(strata_core_server::serve(a.bind, data_dir, a.web_dir))
}
