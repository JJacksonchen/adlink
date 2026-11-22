//! Lock-free Prometheus text-format metrics registry for AdLink commercial server deployments.

use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use dashmap::DashMap;

const LATENCY_BUCKETS_SECS: [f64; 11] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

/// Lock-free runtime telemetry registry exported via `GET /metrics`.
pub struct MetricsRegistry {
    pub http_requests_in_flight: AtomicU64,
    pub rate_limited_total: AtomicU64,
    pub load_shed_total: AtomicU64,
    pub timeouts_total: AtomicU64,
    pub cache_hits_total: AtomicU64,
    pub cache_misses_total: AtomicU64,
    pub idempotency_replays_total: AtomicU64,
    pub jobs_enqueued_total: AtomicU64,
    pub jobs_completed_total: AtomicU64,
    pub jobs_failed_total: AtomicU64,
    pub jobs_in_flight: AtomicU64,
    pub start_time_unix: u64,
    request_counts: DashMap<(String, String, u16), AtomicU64>,
    duration_buckets: [AtomicU64; 12],
    duration_sum_micros: AtomicU64,
    duration_count: AtomicU64,
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsRegistry {
    pub fn new() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            http_requests_in_flight: AtomicU64::new(0),
            rate_limited_total: AtomicU64::new(0),
            load_shed_total: AtomicU64::new(0),
            timeouts_total: AtomicU64::new(0),
            cache_hits_total: AtomicU64::new(0),
            cache_misses_total: AtomicU64::new(0),
            idempotency_replays_total: AtomicU64::new(0),
            jobs_enqueued_total: AtomicU64::new(0),
            jobs_completed_total: AtomicU64::new(0),
            jobs_failed_total: AtomicU64::new(0),
            jobs_in_flight: AtomicU64::new(0),
            start_time_unix: now,
            request_counts: DashMap::new(),
            duration_buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            duration_sum_micros: AtomicU64::new(0),
            duration_count: AtomicU64::new(0),
        }
    }

    /// Normalizes dynamic path segments into low-cardinality route groups for Prometheus labels.
    pub fn classify_route_group(path: &str) -> &'static str {
        if path == "/healthz" {
            "/healthz"
        } else if path == "/readyz" {
            "/readyz"
        } else if path == "/metrics" {
            "/metrics"
        } else if path == "/mcp" || path == "/sse" || path == "/messages" {
            "/mcp"
        } else if path.starts_with("/api/v1/analytics/") {
            "/api/v1/analytics"
        } else if path.starts_with("/api/v1/attribution/") {
            "/api/v1/attribution"
        } else if path.starts_with("/api/v1/sync/") {
            "/api/v1/sync"
        } else if path.starts_with("/api/v1/platforms") {
            "/api/v1/platforms"
        } else if path.starts_with("/api/v1/governance/") {
            "/api/v1/governance"
        } else if path.starts_with("/api/v1/ad-creation/") {
            "/api/v1/ad-creation"
        } else if path.starts_with("/api/v1/ad-ops/") {
            "/api/v1/ad-ops"
        } else if path.starts_with("/api/v1/") {
            "/api/v1/other"
        } else {
            "/static"
        }
    }

    /// Records one completed HTTP request's counter and latency histogram observation.
    pub fn record_http_request(&self, method: &str, path: &str, status: u16, elapsed_secs: f64) {
        let route_group = Self::classify_route_group(path).to_string();
        let key = (method.to_string(), route_group, status);
        if let Some(counter) = self.request_counts.get(&key) {
            counter.fetch_add(1, Ordering::Relaxed);
        } else {
            self.request_counts
                .entry(key)
                .or_insert_with(|| AtomicU64::new(0))
                .fetch_add(1, Ordering::Relaxed);
        }

        let micros = (elapsed_secs * 1_000_000.0).round().max(0.0) as u64;
        self.duration_sum_micros.fetch_add(micros, Ordering::Relaxed);
        self.duration_count.fetch_add(1, Ordering::Relaxed);

        for (i, &bound) in LATENCY_BUCKETS_SECS.iter().enumerate() {
            if elapsed_secs <= bound {
                self.duration_buckets[i].fetch_add(1, Ordering::Relaxed);
            }
        }
        self.duration_buckets[LATENCY_BUCKETS_SECS.len()].fetch_add(1, Ordering::Relaxed);
    }

    /// Renders Prometheus text exposition (`version=0.0.4`).
    pub fn render_prometheus(
        &self,
        storage_epoch: u64,
        storage_pool_readers: usize,
        queue_depth: usize,
        draining: bool,
    ) -> String {
        let mut out = String::with_capacity(2048);

        let _ = writeln!(out, "# HELP adlink_process_start_time_seconds Start time of AdLink server since Unix epoch.");
        let _ = writeln!(out, "# TYPE adlink_process_start_time_seconds gauge");
        let _ = writeln!(out, "adlink_process_start_time_seconds {}", self.start_time_unix);

        let _ = writeln!(out, "# HELP adlink_server_draining Whether the server is draining prior to shutdown (1=draining, 0=serving).");
        let _ = writeln!(out, "# TYPE adlink_server_draining gauge");
        let _ = writeln!(out, "adlink_server_draining {}", if draining { 1 } else { 0 });

        let _ = writeln!(out, "# HELP adlink_http_requests_in_flight Current number of active HTTP requests.");
        let _ = writeln!(out, "# TYPE adlink_http_requests_in_flight gauge");
        let _ = writeln!(out, "adlink_http_requests_in_flight {}", self.http_requests_in_flight.load(Ordering::Relaxed));

        let _ = writeln!(out, "# HELP adlink_http_requests_total Total HTTP requests partitioned by method, route group, and status.");
        let _ = writeln!(out, "# TYPE adlink_http_requests_total counter");
        let mut sorted: Vec<((String, String, u16), u64)> = self
            .request_counts
            .iter()
            .map(|e| (e.key().clone(), e.value().load(Ordering::Relaxed)))
            .collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        for ((method, route, status), val) in sorted {
            let _ = writeln!(
                out,
                "adlink_http_requests_total{{method=\"{method}\",route=\"{route}\",status=\"{status}\"}} {val}"
            );
        }

        let _ = writeln!(out, "# HELP adlink_http_request_duration_seconds HTTP request latency histogram.");
        let _ = writeln!(out, "# TYPE adlink_http_request_duration_seconds histogram");
        for (i, &bound) in LATENCY_BUCKETS_SECS.iter().enumerate() {
            let count = self.duration_buckets[i].load(Ordering::Relaxed);
            let _ = writeln!(
                out,
                "adlink_http_request_duration_seconds_bucket{{le=\"{bound}\"}} {count}"
            );
        }
        let inf_count = self.duration_buckets[LATENCY_BUCKETS_SECS.len()].load(Ordering::Relaxed);
        let _ = writeln!(out, "adlink_http_request_duration_seconds_bucket{{le=\"+Inf\"}} {inf_count}");
        let sum_secs = self.duration_sum_micros.load(Ordering::Relaxed) as f64 / 1_000_000.0;
        let _ = writeln!(out, "adlink_http_request_duration_seconds_sum {sum_secs:.6}");
        let _ = writeln!(
            out,
            "adlink_http_request_duration_seconds_count {}",
            self.duration_count.load(Ordering::Relaxed)
        );

        let _ = writeln!(out, "# HELP adlink_rate_limited_total Total requests rejected with HTTP 429.");
        let _ = writeln!(out, "# TYPE adlink_rate_limited_total counter");
        let _ = writeln!(out, "adlink_rate_limited_total {}", self.rate_limited_total.load(Ordering::Relaxed));

        let _ = writeln!(out, "# HELP adlink_load_shed_total Total requests shed with HTTP 503 when max_in_flight saturated.");
        let _ = writeln!(out, "# TYPE adlink_load_shed_total counter");
        let _ = writeln!(out, "adlink_load_shed_total {}", self.load_shed_total.load(Ordering::Relaxed));

        let _ = writeln!(out, "# HELP adlink_timeouts_total Total requests terminated with HTTP 504 on timeout.");
        let _ = writeln!(out, "# TYPE adlink_timeouts_total counter");
        let _ = writeln!(out, "adlink_timeouts_total {}", self.timeouts_total.load(Ordering::Relaxed));

        let _ = writeln!(out, "# HELP adlink_cache_hits_total Analytics query cache hits.");
        let _ = writeln!(out, "# TYPE adlink_cache_hits_total counter");
        let _ = writeln!(out, "adlink_cache_hits_total {}", self.cache_hits_total.load(Ordering::Relaxed));

        let _ = writeln!(out, "# HELP adlink_cache_misses_total Analytics query cache misses.");
        let _ = writeln!(out, "# TYPE adlink_cache_misses_total counter");
        let _ = writeln!(out, "adlink_cache_misses_total {}", self.cache_misses_total.load(Ordering::Relaxed));

        let _ = writeln!(out, "# HELP adlink_idempotency_replays_total Mutating requests replayed from Idempotency-Key cache.");
        let _ = writeln!(out, "# TYPE adlink_idempotency_replays_total counter");
        let _ = writeln!(out, "adlink_idempotency_replays_total {}", self.idempotency_replays_total.load(Ordering::Relaxed));

        let _ = writeln!(out, "# HELP adlink_jobs_enqueued_total Total async sync jobs enqueued.");
        let _ = writeln!(out, "# TYPE adlink_jobs_enqueued_total counter");
        let _ = writeln!(out, "adlink_jobs_enqueued_total {}", self.jobs_enqueued_total.load(Ordering::Relaxed));

        let _ = writeln!(out, "# HELP adlink_jobs_completed_total Total async sync jobs completed.");
        let _ = writeln!(out, "# TYPE adlink_jobs_completed_total counter");
        let _ = writeln!(out, "adlink_jobs_completed_total {}", self.jobs_completed_total.load(Ordering::Relaxed));

        let _ = writeln!(out, "# HELP adlink_jobs_failed_total Total async sync jobs failed.");
        let _ = writeln!(out, "# TYPE adlink_jobs_failed_total counter");
        let _ = writeln!(out, "adlink_jobs_failed_total {}", self.jobs_failed_total.load(Ordering::Relaxed));

        let _ = writeln!(out, "# HELP adlink_jobs_in_flight Current running async sync jobs.");
        let _ = writeln!(out, "# TYPE adlink_jobs_in_flight gauge");
        let _ = writeln!(out, "adlink_jobs_in_flight {}", self.jobs_in_flight.load(Ordering::Relaxed));

        let _ = writeln!(out, "# HELP adlink_jobs_queue_depth Approximate queued async sync jobs waiting for workers.");
        let _ = writeln!(out, "# TYPE adlink_jobs_queue_depth gauge");
        let _ = writeln!(out, "adlink_jobs_queue_depth {queue_depth}");

        let _ = writeln!(out, "# HELP adlink_storage_write_epoch Monotonic SQLite WAL committed write epoch.");
        let _ = writeln!(out, "# TYPE adlink_storage_write_epoch counter");
        let _ = writeln!(out, "adlink_storage_write_epoch {storage_epoch}");

        let _ = writeln!(out, "# HELP adlink_storage_reader_pool_size Configured SQLite WAL read-only pool connections.");
        let _ = writeln!(out, "# TYPE adlink_storage_reader_pool_size gauge");
        let _ = writeln!(out, "adlink_storage_reader_pool_size {storage_pool_readers}");

        out
    }
}
