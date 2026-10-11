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
    /// Show where the config file lives and this install's Plex client id.
    Config,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let store = ConfigStore::from_env()?;

    match cli.command {
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
