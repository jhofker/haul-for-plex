//! Retrying downloads when a server drops, with exponential backoff.

use std::future::Future;
use std::path::Path;
use std::time::Duration;

use crate::download::{Outcome, Progress};
use crate::error::{Error, Result};
use crate::library::Part;
use crate::server::ServerClient;

/// How often and how patiently to retry a failed download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Retries after the first attempt; 0 disables retrying.
    pub retries: u32,
    /// Wait before the first retry; doubled for each one after.
    pub first_delay: Duration,
    /// Longest wait between attempts.
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            retries: 5,
            first_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(60),
        }
    }
}

impl RetryPolicy {
    /// Wait before retry number `retry` (1 for the first).
    pub fn delay(&self, retry: u32) -> Duration {
        let factor = 1u32 << retry.saturating_sub(1).min(16);
        self.first_delay.saturating_mul(factor).min(self.max_delay)
    }
}

/// Download `part` to `dest` like [`ServerClient::download_part`], retrying
/// errors that [`Error::is_retryable`] up to `policy.retries` times.
///
/// `connect(attempt)` supplies the client for each attempt (0 for the first);
/// on a retry it should connect afresh, since the best connection (local,
/// remote, relay) may have changed. Each retry resumes from the partial file.
/// `on_retry` is told the error, the wait, and the retry number before waiting.
pub async fn download_with_retry<C, F>(
    mut connect: C,
    part: &Part,
    dest: &Path,
    overwrite: bool,
    policy: &RetryPolicy,
    mut on_progress: impl FnMut(Progress),
    mut on_retry: impl FnMut(&Error, Duration, u32),
) -> Result<Outcome>
where
    C: FnMut(u32) -> F,
    F: Future<Output = Result<ServerClient>>,
{
    let mut attempt = 0;
    loop {
        let result = match connect(attempt).await {
            Ok(client) => {
                client
                    .download_part(part, dest, overwrite, &mut on_progress)
                    .await
            }
            Err(e) => Err(e),
        };
        match result {
            Err(e) if e.is_retryable() && attempt < policy.retries => {
                attempt += 1;
                let wait = policy.delay(attempt);
                on_retry(&e, wait, attempt);
                tokio::time::sleep(wait).await;
            }
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_doubles_up_to_the_cap() {
        let policy = RetryPolicy {
            retries: 10,
            first_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(30),
        };
        let delays: Vec<_> = (1..=6).map(|n| policy.delay(n).as_secs()).collect();
        assert_eq!(delays, [2, 4, 8, 16, 30, 30]);
        assert_eq!(policy.delay(1000), Duration::from_secs(30));
    }
}
