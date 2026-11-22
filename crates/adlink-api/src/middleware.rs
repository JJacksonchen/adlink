//! Production HTTP guardrails: X-Request-Id, security headers, timeout, adaptive load shedding,
//! and per-tenant token-bucket rate limiting.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use dashmap::DashMap;
use serde_json::json;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::metrics::MetricsRegistry;
use crate::server_config::ProductionServerConfig;

struct BucketState {
    tokens: f64,
    last_refill: Instant,
}

/// Concurrent per-tenant / per-IP token bucket rate limiter.
pub struct TokenBucketRateLimiter {
    rps: f64,
    burst: f64,
    buckets: DashMap<String, BucketState>,
}

impl TokenBucketRateLimiter {
    pub fn new(rps: u32, burst: u32) -> Self {
        Self {
            rps: rps as f64,
            burst: burst as f64,
            buckets: DashMap::new(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.rps > 0.0 && self.burst > 0.0
    }

    /// Consumes 1 token for `tenant_key`, returning `(allowed, remaining_tokens)`.
    pub fn check(&self, tenant_key: &str) -> (bool, u32) {
        if !self.is_enabled() {
            return (true, u32::MAX);
        }
        let now = Instant::now();
        if self.buckets.len() > 8192 {
            // Opportunistically clean idle buckets inactive for > 10 minutes.
            self.buckets
                .retain(|_, b| now.duration_since(b.last_refill) < Duration::from_secs(600));
        }

        let mut entry = self
            .buckets
            .entry(tenant_key.to_string())
            .or_insert_with(|| BucketState {
                tokens: self.burst,
                last_refill: now,
            });
        let elapsed = now.duration_since(entry.last_refill).as_secs_f64();
        if elapsed > 0.0 {
            entry.tokens = (entry.tokens + elapsed * self.rps).min(self.burst);
            entry.last_refill = now;
        }
        if entry.tokens >= 1.0 {
            entry.tokens -= 1.0;
            (true, entry.tokens.floor().max(0.0) as u32)
        } else {
            (false, 0)
        }
    }
}

#[derive(Clone)]
pub struct MiddlewareState {
    pub config: Arc<ProductionServerConfig>,
    pub metrics: Arc<MetricsRegistry>,
    pub in_flight_sem: Arc<Semaphore>,
    pub rate_limiter: Arc<TokenBucketRateLimiter>,
}

impl MiddlewareState {
    pub fn new(config: Arc<ProductionServerConfig>, metrics: Arc<MetricsRegistry>) -> Self {
        let sem = Arc::new(Semaphore::new(config.max_in_flight_requests));
        let limiter = Arc::new(TokenBucketRateLimiter::new(
            config.rate_limit_rps,
            config.rate_limit_burst,
        ));
        Self {
            config,
            metrics,
            in_flight_sem: sem,
            rate_limiter: limiter,
        }
    }
}

fn is_probe_path(path: &str) -> bool {
    matches!(path, "/healthz" | "/readyz" | "/metrics")
}

struct InFlightGuard {
    metrics: Arc<MetricsRegistry>,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.metrics
            .http_requests_in_flight
            .fetch_sub(1, Ordering::Relaxed);
    }
}

/// Extracts tenant identity for rate-limiting: API key token if provided, else client IP headers.
fn extract_tenant_key(req: &Request<Body>) -> String {
    if let Some(auth) = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return format!("key:{auth}");
    }
    if let Some(key) = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return format!("key:{key}");
    }
    if let Some(ip) = req
        .headers()
        .get("x-forwarded-for")
        .or_else(|| req.headers().get("x-real-ip"))
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return format!("ip:{ip}");
    }
    "anonymous".into()
}

/// Attaches `X-Request-Id`, security headers, skips timeout for `/sse`, enforces request timeout,
/// and records Prometheus HTTP request counters + latency histogram.
pub async fn observability_and_timeout_middleware(
    State(state): State<MiddlewareState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let started = Instant::now();
    state
        .metrics
        .http_requests_in_flight
        .fetch_add(1, Ordering::Relaxed);
    let _guard = InFlightGuard {
        metrics: Arc::clone(&state.metrics),
    };

    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();
    let req_id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let skip_timeout = path == "/sse" || is_probe_path(&path);
    let mut res = if skip_timeout || state.config.request_timeout_secs == 0 {
        next.run(req).await
    } else {
        let timeout = Duration::from_secs(state.config.request_timeout_secs);
        match tokio::time::timeout(timeout, next.run(req)).await {
            Ok(resp) => resp,
            Err(_) => {
                state.metrics.timeouts_total.fetch_add(1, Ordering::Relaxed);
                (
                    StatusCode::GATEWAY_TIMEOUT,
                    axum::Json(json!({
                        "error": "request_timeout",
                        "message": format!("Request exceeded {}s deadline.", timeout.as_secs())
                    })),
                )
                    .into_response()
            }
        }
    };

    let headers = res.headers_mut();
    if let Ok(val) = HeaderValue::from_str(&req_id) {
        headers.insert("x-request-id", val);
    }
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );

    let elapsed = started.elapsed().as_secs_f64();
    state
        .metrics
        .record_http_request(&method, &path, res.status().as_u16(), elapsed);
    res
}

/// Adaptive load shedding: rejects excess requests with HTTP 503 when `max_in_flight_requests` is saturated.
pub async fn load_shed_middleware(
    State(state): State<MiddlewareState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if is_probe_path(req.uri().path()) {
        return next.run(req).await;
    }
    let Ok(_permit) = state.in_flight_sem.clone().try_acquire_owned() else {
        state.metrics.load_shed_total.fetch_add(1, Ordering::Relaxed);
        let mut resp = (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(json!({
                "error": "server_overloaded",
                "message": "Server at max_in_flight_requests capacity; retry shortly."
            })),
        )
            .into_response();
        resp.headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        return resp;
    };
    next.run(req).await
}

/// Per-tenant / per-IP token-bucket rate limiter returning HTTP 429 on burst exhaustion.
pub async fn rate_limit_middleware(
    State(state): State<MiddlewareState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if is_probe_path(req.uri().path()) || !state.rate_limiter.is_enabled() {
        return next.run(req).await;
    }
    let tenant_key = extract_tenant_key(&req);
    let (allowed, remaining) = state.rate_limiter.check(&tenant_key);
    if !allowed {
        state
            .metrics
            .rate_limited_total
            .fetch_add(1, Ordering::Relaxed);
        let mut resp = (
            StatusCode::TOO_MANY_REQUESTS,
            axum::Json(json!({
                "error": "rate_limit_exceeded",
                "message": "Too many requests; respect Retry-After header."
            })),
        )
            .into_response();
        let headers = resp.headers_mut();
        headers.insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("0"));
        return resp;
    }
    let mut resp = next.run(req).await;
    if let Ok(val) = HeaderValue::from_str(&remaining.to_string()) {
        resp.headers_mut().insert("x-ratelimit-remaining", val);
    }
    resp
}
