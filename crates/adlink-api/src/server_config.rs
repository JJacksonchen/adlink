//! Commercial production server tuning configuration.

use serde::{Deserialize, Serialize};

/// Production HTTP/MCP server guardrails, backpressure limits, caching, and worker settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductionServerConfig {
    /// Maximum concurrent HTTP requests accepted before adaptive load shedding returns `503`.
    #[serde(default = "default_max_in_flight_requests")]
    pub max_in_flight_requests: usize,
    /// Per-request execution timeout in seconds (skipped for long-lived `/sse` streams).
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,
    /// Maximum HTTP request payload size in bytes (`DefaultBodyLimit`).
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: usize,
    /// Steady-state sustained requests/sec per tenant/API-key/IP token bucket.
    #[serde(default = "default_rate_limit_rps")]
    pub rate_limit_rps: u32,
    /// Token bucket burst capacity per tenant/API-key/IP.
    #[serde(default = "default_rate_limit_burst")]
    pub rate_limit_burst: u32,
    /// Analytics query cache TTL in seconds (`0` disables caching).
    #[serde(default = "default_analytics_cache_ttl_secs")]
    pub analytics_cache_ttl_secs: u64,
    /// Maximum cached analytics responses stored in memory.
    #[serde(default = "default_analytics_cache_max_entries")]
    pub analytics_cache_max_entries: usize,
    /// Idempotency-Key deduplication window in seconds.
    #[serde(default = "default_idempotency_ttl_secs")]
    pub idempotency_ttl_secs: u64,
    /// Bounded capacity for `/api/v1/sync/jobs/enqueue`.
    #[serde(default = "default_job_queue_capacity")]
    pub job_queue_capacity: usize,
    /// Number of background sync job workers pulling from the bounded queue.
    #[serde(default = "default_job_worker_concurrency")]
    pub job_worker_concurrency: usize,
    /// Optional periodic background sync interval across all platforms (`0` disables scheduler).
    #[serde(default)]
    pub scheduled_sync_interval_secs: u64,
    /// Optional webhook URL notified on async sync job completion/failure.
    #[serde(default)]
    pub webhook_url: Option<String>,
    /// Allowed CORS origins (empty list allows any origin).
    #[serde(default)]
    pub cors_allowed_origins: Vec<String>,
    /// Log output format (`"text"` or `"json"`), always routed to `stderr`.
    #[serde(default = "default_log_format")]
    pub log_format: String,
    /// Graceful HTTP/worker drain window in seconds on `SIGTERM` / `SIGINT`.
    #[serde(default = "default_shutdown_drain_secs")]
    pub shutdown_drain_secs: u64,
}

impl Default for ProductionServerConfig {
    fn default() -> Self {
        Self {
            max_in_flight_requests: default_max_in_flight_requests(),
            request_timeout_secs: default_request_timeout_secs(),
            max_body_bytes: default_max_body_bytes(),
            rate_limit_rps: default_rate_limit_rps(),
            rate_limit_burst: default_rate_limit_burst(),
            analytics_cache_ttl_secs: default_analytics_cache_ttl_secs(),
            analytics_cache_max_entries: default_analytics_cache_max_entries(),
            idempotency_ttl_secs: default_idempotency_ttl_secs(),
            job_queue_capacity: default_job_queue_capacity(),
            job_worker_concurrency: default_job_worker_concurrency(),
            scheduled_sync_interval_secs: 0,
            webhook_url: None,
            cors_allowed_origins: Vec::new(),
            log_format: default_log_format(),
            shutdown_drain_secs: default_shutdown_drain_secs(),
        }
    }
}

fn default_max_in_flight_requests() -> usize {
    512
}
fn default_request_timeout_secs() -> u64 {
    30
}
fn default_max_body_bytes() -> usize {
    16 * 1024 * 1024
}
fn default_rate_limit_rps() -> u32 {
    120
}
fn default_rate_limit_burst() -> u32 {
    240
}
fn default_analytics_cache_ttl_secs() -> u64 {
    30
}
fn default_analytics_cache_max_entries() -> usize {
    1024
}
fn default_idempotency_ttl_secs() -> u64 {
    600
}
fn default_job_queue_capacity() -> usize {
    128
}
fn default_job_worker_concurrency() -> usize {
    4
}
fn default_log_format() -> String {
    "text".into()
}
fn default_shutdown_drain_secs() -> u64 {
    15
}
