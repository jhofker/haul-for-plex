use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use futures_util::StreamExt;
use futures_util::stream;
use haul_core::{Error, Layout, Outcome, PlannedFile, RetryPolicy};
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};

use crate::context::Context;
use crate::table;

pub struct Options {
    pub server: String,
    pub key: String,
    pub movies_dir: Option<PathBuf>,
    pub tv_dir: Option<PathBuf>,
    pub parallel: usize,
    pub version: haul_core::VersionChoice,
    pub dry_run: bool,
    pub force: bool,
    pub json: bool,
    /// Retries per file when the server drops or answers with a temporary error.
    pub retries: u32,
}

/// Movie and TV folders, defaulting to `./downloads/Movies` and `./downloads/TV Shows`.
pub fn layout(movies_dir: Option<PathBuf>, tv_dir: Option<PathBuf>) -> Layout {
    let downloads = PathBuf::from("downloads");
    Layout {
        movies_dir: movies_dir.unwrap_or_else(|| downloads.join("Movies")),
        tv_dir: tv_dir.unwrap_or_else(|| downloads.join("TV Shows")),
    }
}

#[derive(Default)]
struct Tally {
    downloaded: usize,
    skipped: usize,
    failed: usize,
    not_attempted: usize,
}

pub async fn run(mut ctx: Context, opts: Options) -> Result<()> {
    let (resource, client) = ctx.server_with_resource(&opts.server).await?;
    let policy = RetryPolicy {
        retries: opts.retries,
        ..RetryPolicy::default()
    };
    let item = client.metadata(&opts.key).await?;

    let layout = layout(opts.movies_dir, opts.tv_dir);
    let files = client
        .plan_download_version(&item, &layout, &opts.version)
        .await?;

    let total: u64 = files.iter().filter_map(|f| f.part.size).sum();
    if opts.dry_run && opts.json {
        let entries: Vec<_> = files
            .iter()
            .map(|f| serde_json::json!({"path": f.dest, "size": f.part.size}))
            .collect();
        crate::output::json(&entries);
        return Ok(());
    }
    if opts.dry_run {
        for file in &files {
            let size = file.part.size.map(table::size).unwrap_or_default();
            println!("{}  {size}", file.dest.display());
        }
        if files.len() > 1 {
            println!("\n{} files, {}", files.len(), table::size(total));
        }
        return Ok(());
    }

    let have = files.iter().filter(|f| already_complete(f)).count();
    if files.len() > 1 && !opts.json {
        println!(
            "Downloading {} files ({}){}",
            files.len(),
            table::size(total),
            if have > 0 {
                format!(", {have} already present")
            } else {
                String::new()
            }
        );
    }

    let style = ProgressStyle::with_template(
        "{msg}\n{bar:40.cyan/blue} {bytes}/{total_bytes}  {bytes_per_sec}  ETA {eta}",
    )?;
    let multi = if opts.json {
        MultiProgress::with_draw_target(ProgressDrawTarget::hidden())
    } else {
        MultiProgress::new()
    };
    // Set when the server refuses downloads outright: the rest would fail too.
    let refused = AtomicBool::new(false);

    let results: Vec<_> = stream::iter(&files)
        .map(|file| {
            let (client, multi, style, refused) = (&client, &multi, &style, &refused);
            let (ctx, resource, policy) = (&ctx, &resource, &policy);
            async move {
                if refused.load(Ordering::Relaxed) {
                    return (file, None);
                }
                let bar = multi
                    .add(ProgressBar::new(file.part.size.unwrap_or(0)).with_style(style.clone()));
                bar.set_message(file_name(file));
                bar.enable_steady_tick(Duration::from_millis(250));
                let name = file_name(file);
                let result = haul_core::download_with_retry(
                    // Retries connect afresh: the best connection may have changed.
                    |attempt| async move {
                        if attempt == 0 {
                            Ok(client.clone())
                        } else {
                            ctx.connect(resource).await
                        }
                    },
                    &file.part,
                    &file.dest,
                    opts.force,
                    policy,
                    |p| {
                        if let Some(total) = p.total {
                            bar.set_length(total);
                        }
                        bar.set_position(p.downloaded);
                    },
                    |e, wait, n| {
                        let _ = multi.println(format!(
                            "{name}: {e}; retrying in {}s ({n} of {})",
                            wait.as_secs(),
                            policy.retries
                        ));
                    },
                )
                .await;
                bar.finish_and_clear();
                multi.remove(&bar);
                if matches!(result, Err(Error::DownloadForbidden(_))) {
                    refused.store(true, Ordering::Relaxed);
                }
                (file, Some(result))
            }
        })
        .buffer_unordered(opts.parallel.max(1))
        .collect()
        .await;

    // Report in plan order rather than completion order.
    let mut tally = Tally::default();
    let mut failures = Vec::new();
    let mut entries = Vec::new();
    for file in &files {
        let Some((_, result)) = results.iter().find(|(f, _)| std::ptr::eq(*f, file)) else {
            continue;
        };
        match result {
            None => {
                tally.not_attempted += 1;
                entries.push(serde_json::json!({"path": file.dest, "status": "not_attempted"}));
            }
            Some(Ok(Outcome::Skipped)) => {
                tally.skipped += 1;
                entries.push(serde_json::json!({"path": file.dest, "status": "skipped"}));
                if !opts.json {
                    println!("Already have {}", file.dest.display());
                }
            }
            Some(Ok(Outcome::Downloaded { resumed_from })) => {
                tally.downloaded += 1;
                let size = std::fs::metadata(&file.dest).map(|m| m.len()).unwrap_or(0);
                let resumed = if *resumed_from > 0 {
                    format!(" (resumed from {})", table::size(*resumed_from))
                } else {
                    String::new()
                };
                println!(
                    "Saved {} ({}){resumed}",
                    file.dest.display(),
                    table::size(size)
                );
            }
            Some(Err(e)) => {
                tally.failed += 1;
                entries.push(serde_json::json!({
                    "path": file.dest, "status": "failed", "error": e.to_string(),
                }));
                failures.push(format!("{}: {e}", file.dest.display()));
            }
        }
    }

    if opts.json {
        crate::output::json(&entries);
    } else if files.len() > 1 {
        println!(
            "\nDone: {} downloaded, {} already present, {} failed",
            tally.downloaded, tally.skipped, tally.failed
        );
    }
    for failure in &failures {
        eprintln!("error: {failure}");
    }
    if tally.not_attempted > 0 {
        eprintln!(
            "{} files were not attempted because the server refused downloads",
            tally.not_attempted
        );
    }
    if tally.failed > 0 || tally.not_attempted > 0 {
        anyhow::bail!(
            "{} of {} files did not download",
            tally.failed + tally.not_attempted,
            files.len()
        );
    }
    Ok(())
}

fn file_name(file: &PlannedFile) -> String {
    file.dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn already_complete(file: &PlannedFile) -> bool {
    match (std::fs::metadata(&file.dest), file.part.size) {
        (Ok(meta), Some(size)) => meta.len() == size,
        _ => false,
    }
}
