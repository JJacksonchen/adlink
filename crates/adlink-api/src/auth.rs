//! Optional Bearer / X-Api-Key authentication middleware.

use std::collections::HashSet;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Clone, Default)]
pub struct ApiAuthConfig {
    pub api_keys: Arc<HashSet<String>>,
}

impl ApiAuthConfig {
    pub fn from_keys(keys: Vec<String>) -> Self {
        let filtered: HashSet<String> = keys
            .into_iter()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .collect();
        Self {
            api_keys: Arc::new(filtered),
        }
    }

    pub fn is_enabled(&self) -> bool {
        !self.api_keys.is_empty()
    }
}

pub async fn api_key_middleware(
    axum::extract::State(auth): axum::extract::State<ApiAuthConfig>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if !auth.is_enabled() {
        return next.run(req).await;
    }

    let path = req.uri().path();
    // Keep Kubernetes/Prometheus probes, legacy health, OpenAPI discovery, and static Web starter open.
    if path == "/healthz"
        || path == "/readyz"
        || path == "/metrics"
        || path == "/api/v1/health"
        || path == "/api/v1/openapi.json"
        || (!path.starts_with("/api/")
            && path != "/mcp"
            && path != "/sse"
            && path != "/messages")
    {
        return next.run(req).await;
    }

    let bearer = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::trim);

    let x_api_key = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim);

    let token = bearer.or(x_api_key);
    match token {
        Some(t) if auth.api_keys.contains(t) => next.run(req).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({
                "error": "unauthorized",
                "message": "Provide a valid Authorization: Bearer <key> or X-Api-Key header."
            })),
        )
            .into_response(),
    }
}
