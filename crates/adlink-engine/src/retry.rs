//! Adaptive retry policy with exponential backoff and deterministic jitter.

use std::future::Future;
use std::time::Duration;

use crate::connector::ConnectorError;

/// Configurable retry policy for transient upstream platform errors and HTTP 429 throttles.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            base_delay_ms: 50,
            max_delay_ms: 4_000,
        }
    }
}

impl RetryPolicy {
    /// Computes backoff duration for `attempt` (1-indexed), honoring server `retry_after_ms` when given.
    pub fn delay_for_attempt(&self, attempt: u32, retry_after_ms: Option<u64>) -> Duration {
        if let Some(server_hint) = retry_after_ms {
            return Duration::from_millis(server_hint.min(self.max_delay_ms));
        }
        let exp = 2u64.saturating_pow(attempt.saturating_sub(1).min(10));
        let raw = self.base_delay_ms.saturating_mul(exp);
        // Deterministic pseudo-jitter in [0..base_delay_ms/2] avoids thundering-herd without rand dep.
        let jitter = (u64::from(attempt) * 17) % (self.base_delay_ms / 2 + 1);
        Duration::from_millis((raw + jitter).min(self.max_delay_ms))
    }

    /// Executes `op` up to `max_attempts`, retrying only retryable [`ConnectorError`] variants.
    pub async fn execute<F, Fut, T>(&self, mut op: F) -> Result<T, ConnectorError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, ConnectorError>>,
    {
        let max = self.max_attempts.max(1);
        let mut attempt = 1;
        loop {
            match op().await {
                Ok(v) => return Ok(v),
                Err(err) if attempt < max && err.is_retryable() => {
                    let wait = self.delay_for_attempt(attempt, err.retry_after_ms());
                    tracing::warn!(
                        attempt,
                        max_attempts = max,
                        wait_ms = wait.as_millis() as u64,
                        error = %err,
                        "retrying transient connector error"
                    );
                    tokio::time::sleep(wait).await;
                    attempt += 1;
                }
                Err(err) => return Err(err),
            }
        }
    }
}
