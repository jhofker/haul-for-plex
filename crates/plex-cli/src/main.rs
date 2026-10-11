mod auth;
mod browse;
mod context;
mod download;
mod search;
mod servers;
mod table;

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
    /// List the Plex servers you can access and how each is reachable.
    Servers {
        /// Show the connection used, or why each connection failed.
        #[arg(short, long)]
        verbose: bool,
    },
    /// List the libraries on a server.
    Libraries {
        /// Server name (or the start of it) or machine id.
        server: String,
    },
    /// List a library's items, a show's seasons, a season's episodes, or an item's files.
    Ls {
        /// Server name (or the start of it) or machine id.
        server: String,
        /// Library name, or an item key from a previous listing.
        target: String,
    },
    /// Search for movies, shows and episodes across your servers.
    Search {
        /// What to search for.
        query: String,
        /// Only search this server (name, start of name, or machine id).
        #[arg(short, long)]
        server: Option<String>,
        /// Maximum results per type on each server.
        #[arg(short, long, default_value_t = 20)]
        limit: usize,
    },
    /// Download a movie or episode into the Plex-style folder layout.
    ///
    /// Interrupted downloads resume where they left off when you run the
    /// same command again.
    Download {
        /// Server name (or the start of it) or machine id.
        server: String,
        /// Item key from `ls` or `search`.
        key: String,
        /// Where movies are saved.
        #[arg(long, env = "PLEX_GRABBER_MOVIES_DIR", value_name = "DIR")]
        movies_dir: Option<std::path::PathBuf>,
        /// Where TV episodes are saved.
        #[arg(long, env = "PLEX_GRABBER_TV_DIR", value_name = "DIR")]
        tv_dir: Option<std::path::PathBuf>,
        /// Show where the files would be saved without downloading.
        #[arg(long)]
        dry_run: bool,
        /// Replace an existing file that has a different size.
        #[arg(long)]
        force: bool,
    },
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
        Command::Servers { verbose } => {
            servers::list(context::Context::load(store)?, verbose).await?
        }
        Command::Libraries { server } => {
            browse::libraries(context::Context::load(store)?, &server).await?
        }
        Command::Ls { server, target } => {
            browse::ls(context::Context::load(store)?, &server, &target).await?
        }
        Command::Search {
            query,
            server,
            limit,
        } => {
            search::search(
                context::Context::load(store)?,
                &query,
                server.as_deref(),
                limit,
            )
            .await?
        }
        Command::Download {
            server,
            key,
            movies_dir,
            tv_dir,
            dry_run,
            force,
        } => {
            let options = download::Options {
                server,
                key,
                movies_dir,
                tv_dir,
                dry_run,
                force,
            };
            download::run(context::Context::load(store)?, options).await?
        }
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
