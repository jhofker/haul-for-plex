mod auth;

use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};
use plex_core::ConfigStore;

#[derive(Parser)]
#[command(
    name = "plex-grabber",
    version,
    about = "Download media from Plex servers you have access to"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Sign in to your Plex account in a browser.
    Login {
        /// Sign in again even if already logged in.
        #[arg(long)]
        force: bool,
    },
    /// Forget the saved Plex token.
    Logout,
    /// Show the signed-in Plex account.
    Whoami,
    /// Show where the config file lives and this install's Plex client id.
    Config,
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    let store = ConfigStore::from_env()?;

    match cli.command {
        Command::Login { force } => auth::login(&store, force).await?,
        Command::Logout => auth::logout(&store)?,
        Command::Whoami => auth::whoami(&store).await?,
        Command::Config => {
            let config = store.load_or_init()?;
            println!("config file: {}", store.path().display());
            println!("client id:   {}", config.client_id);
            match &config.auth {
                Some(auth) => println!("logged in:   {}", auth.username),
                None => println!("logged in:   no"),
            }
        }
    }
    Ok(())
}
