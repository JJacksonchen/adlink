//! Hierarchical per-platform and per-account asynchronous token-bucket rate limiter.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::Mutex;

struct BucketState {
    tokens: f64,
    capacity: f64,
    refill_per_sec: f64,
    last_refill: Instant,
}

/// Async token bucket enforcing sustained QPS without spinning CPU.
pub struct TokenBucket {
    state: Mutex<BucketState>,
}

impl TokenBucket {
    /// Creates a token bucket refilling at `qps` tokens/sec with burst `capacity`.
    pub fn new(qps: f64, capacity: f64) -> Self {
        let rate = qps.max(0.2);
        let cap = capacity.max(1.0);
        Self {
            state: Mutex::new(BucketState {
                tokens: cap,
                capacity: cap,
                refill_per_sec: rate,
                last_refill: Instant::now(),
            }),
        }
    }

    /// Acquires 1 permit, sleeping asynchronously until a token refills if exhausted.
    pub async fn acquire(&self) {
        loop {
            let wait_for = {
                let mut guard = self.state.lock().await;
                let now = Instant::now();
                let elapsed = now.duration_since(guard.last_refill).as_secs_f64();
                guard.tokens = (guard.tokens + elapsed * guard.refill_per_sec).min(guard.capacity);
                guard.last_refill = now;

                if guard.tokens >= 1.0 {
                    guard.tokens -= 1.0;
                    None
                } else {
                    let deficit = 1.0 - guard.tokens;
                    Some(Duration::from_secs_f64(deficit / guard.refill_per_sec))
                }
            };

            match wait_for {
                None => return,
                Some(dur) => tokio::time::sleep(dur).await,
            }
        }
    }
}

/// Pool of hierarchical token buckets keyed by `platform` and `platform:account_id`.
#[derive(Default)]
pub struct RateLimiterPool {
    buckets: DashMap<String, Arc<TokenBucket>>,
}

impl RateLimiterPool {
    pub fn new() -> Self {
        Self::default()
    }

    fn bucket_for(&self, key: &str, qps: f64) -> Arc<TokenBucket> {
        if let Some(existing) = self.buckets.get(key) {
            return Arc::clone(&existing);
        }
        let burst = (qps * 2.0).max(2.0);
        let created = Arc::new(TokenBucket::new(qps, burst));
        self.buckets
            .entry(key.to_string())
            .or_insert(created)
            .clone()
    }

    /// Acquires permits sequentially at platform level and account level to protect both
    /// app-wide QPS quotas and individual advertiser account throttles.
    pub async fn acquire_hierarchical(&self, platform: &str, account_id: &str, platform_qps: f64) {
        let plat_bucket = self.bucket_for(platform, platform_qps);
        plat_bucket.acquire().await;

        let acct_key = format!("{platform}:{account_id}");
        let acct_bucket = self.bucket_for(&acct_key, (platform_qps * 0.75).max(1.0));
        acct_bucket.acquire().await;
    }
}
