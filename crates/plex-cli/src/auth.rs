use std::time::Duration;

use anyhow::{Context, Result, bail};
use plex_core::{Auth, ClientInfo, ConfigStore, Error, PlexTvClient};

const PIN_POLL_INTERVAL: Duration = Duration::from_secs(2);

pub async fn login(store: &ConfigStore, force: bool) -> Result<()> {
    let mut config = store.load_or_init()?;
    if let Some(auth) = &config.auth
        && !force
    {
        println!(
            "Already logged in as {}. Use `plex-grabber login --force` to sign in again.",
            auth.username
        );
        return Ok(());
    }

    let plex = PlexTvClient::new(ClientInfo::new(&config.client_id))?;
    let pin = plex
        .create_pin()
        .await
        .context("could not start Plex login")?;
    eprintln!("Open this link in a browser and sign in to Plex:\n");
    eprintln!("  {}\n", plex.auth_url(&pin));
    eprintln!("Waiting for approval (Ctrl-C to cancel)...");

    let token = plex.wait_for_pin(&pin, PIN_POLL_INTERVAL).await?;
    let user = plex
        .with_token(&token)
        .user()
        .await
        .context("logged in, but could not fetch the Plex account")?;

    config.auth = Some(Auth {
        token,
        username: user.username.clone(),
    });
    store.save(&config)?;
    println!("Logged in as {}.", user.username);
    Ok(())
}

pub fn logout(store: &ConfigStore) -> Result<()> {
    let mut config = store.load_or_init()?;
    match config.auth.take() {
        Some(auth) => {
            store.save(&config)?;
            println!("Logged out {}.", auth.username);
        }
        None => println!("Not logged in."),
    }
    Ok(())
}

pub async fn whoami(store: &ConfigStore) -> Result<()> {
    let config = store.load_or_init()?;
    let plex = PlexTvClient::new(ClientInfo::new(&config.client_id))?.with_token(config.token()?);
    let user = match plex.user().await {
        Ok(user) => user,
        Err(Error::Status { status, .. }) if status.as_u16() == 401 => {
            bail!("Plex rejected the saved token; run `plex-grabber login --force`")
        }
        Err(e) => return Err(e.into()),
    };

    println!("username: {}", user.username);
    if let Some(email) = user.email.filter(|e| !e.is_empty()) {
        println!("email:    {email}");
    }
    println!("plex id:  {}", user.id);
    Ok(())
}
