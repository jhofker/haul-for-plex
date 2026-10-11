use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use plex_core::{Layout, Outcome};

use crate::context::Context;
use crate::table;

pub struct Options {
    pub server: String,
    pub key: String,
    pub movies_dir: Option<PathBuf>,
    pub tv_dir: Option<PathBuf>,
    pub dry_run: bool,
    pub force: bool,
}

pub async fn run(mut ctx: Context, opts: Options) -> Result<()> {
    let client = ctx.server(&opts.server).await?;
    let item = client.metadata(&opts.key).await?;

    let downloads = PathBuf::from("downloads");
    let layout = Layout {
        movies_dir: opts.movies_dir.unwrap_or_else(|| downloads.join("Movies")),
        tv_dir: opts.tv_dir.unwrap_or_else(|| downloads.join("TV Shows")),
    };
    let files = client.plan_download(&item, &layout).await?;

    if opts.dry_run {
        for file in &files {
            let size = file.part.size.map(table::size).unwrap_or_default();
            println!("{}  {size}", file.dest.display());
        }
        return Ok(());
    }

    let style = ProgressStyle::with_template(
        "{msg}\n{bar:40.cyan/blue} {bytes}/{total_bytes}  {bytes_per_sec}  ETA {eta}",
    )?;
    let mut failed = false;
    for file in &files {
        let name = file
            .dest
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let bar = ProgressBar::new(file.part.size.unwrap_or(0)).with_style(style.clone());
        bar.set_message(name);
        bar.enable_steady_tick(Duration::from_millis(250));

        let result = client
            .download_part(&file.part, &file.dest, opts.force, |p| {
                if let Some(total) = p.total {
                    bar.set_length(total);
                }
                bar.set_position(p.downloaded);
            })
            .await;
        bar.finish_and_clear();

        match result {
            Ok(Outcome::Skipped) => println!("Already have {}", file.dest.display()),
            Ok(Outcome::Downloaded { resumed_from }) => {
                let size = std::fs::metadata(&file.dest).map(|m| m.len()).unwrap_or(0);
                let resumed = if resumed_from > 0 {
                    format!(" (resumed from {})", table::size(resumed_from))
                } else {
                    String::new()
                };
                println!(
                    "Saved {} ({}){resumed}",
                    file.dest.display(),
                    table::size(size)
                );
            }
            Err(e) => {
                eprintln!("error: {}: {e}", file.dest.display());
                failed = true;
                break;
            }
        }
    }
    if failed {
        anyhow::bail!("download did not finish");
    }
    Ok(())
}
