//! Shared HTTP client configuration, credential config types, and official response error mapping.

use std::collections::BTreeMap;
use std::time::Duration;

use adlink_domain::PlatformId;
use adlink_engine::ConnectorError;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Runtime mode for an individual platform connector.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ConnectorMode {
    /// Generates official-schema JSON payloads locally and routes them through the live parser.
    Mock,
    /// Executes live HTTPS requests against the official platform endpoint using configured credentials.
    Live(LiveCredentials),
}

impl Default for ConnectorMode {
    fn default() -> Self {
        Self::Mock
    }
}

/// Unified credential container serialized from `config/adlink.toml` or secret env references.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LiveCredentials {
    #[serde(default)]
    pub app_id: String,
    #[serde(default)]
    pub app_secret: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    /// Alipay RSA2 PKCS#8 private key PEM or base64 DER.
    #[serde(default)]
    pub rsa2_private_key_pem: String,
    /// Alipay Denghuo `biz_token`.
    #[serde(default)]
    pub biz_token: String,
    /// Alipay Denghuo `alipay_pid` (`2088...`).
    #[serde(default)]
    pub alipay_pid: String,
    /// Bilibili commercial `X-Call-Source` header.
    #[serde(default)]
    pub call_source: String,
    /// Optional custom endpoint override (sandbox, proxy gateway, or whitelisted partner URL).
    #[serde(default)]
    pub base_url_override: Option<String>,
    /// Optional extra headers/parameters required by agency gateways.
    #[serde(default)]
    pub extra: BTreeMap<String, String>,
}

/// Builds a rustls-backed reqwest client with sensible commercial timeouts and pooling.
pub fn build_http_client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(16)
        .user_agent("AdLink-Middleware/0.1 (Apache-2.0)")
        .build()
        .unwrap_or_else(|_| Client::new())
}

/// Maps HTTP status codes to retryable vs permanent [`ConnectorError`].
pub fn map_http_status(
    platform: PlatformId,
    status: reqwest::StatusCode,
    body: &str,
    retry_after_header: Option<&str>,
) -> ConnectorError {
    if status.as_u16() == 429 {
        let retry_ms = retry_after_header
            .and_then(|s| s.parse::<u64>().ok())
            .map(|secs| secs * 1000);
        return ConnectorError::RateLimited {
            platform,
            retry_after_ms: retry_ms,
            message: format!("HTTP 429: {body}"),
        };
    }
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return ConnectorError::AuthError {
            platform,
            message: format!("HTTP {}: {body}", status.as_u16()),
        };
    }
    if status.is_server_error() {
        return ConnectorError::Transient {
            platform,
            message: format!("HTTP {}: {body}", status.as_u16()),
        };
    }
    ConnectorError::InvalidRequest {
        platform,
        message: format!("HTTP {}: {body}", status.as_u16()),
    }
}

/// Extracts a `BTreeMap<String, Value>` of raw official fields from a JSON object row.
pub fn extract_raw_map(obj: &Value) -> BTreeMap<String, Value> {
    obj.as_object()
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

pub fn json_u64(obj: &Value, key: &str) -> u64 {
    obj.get(key)
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_i64().map(|n| n.max(0) as u64))
                .or_else(|| v.as_f64().map(|f| f.max(0.0).round() as u64))
                .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok().map(|f| f.max(0.0).round() as u64)))
        })
        .unwrap_or(0)
}

pub fn json_i64(obj: &Value, key: &str) -> i64 {
    obj.get(key)
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_f64().map(|f| f.round() as i64))
                .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok().map(|f| f.round() as i64)))
        })
        .unwrap_or(0)
}

pub fn json_f64(obj: &Value, key: &str) -> f64 {
    obj.get(key)
        .and_then(|v| {
            v.as_f64()
                .or_else(|| v.as_i64().map(|n| n as f64))
                .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
        })
        .unwrap_or(0.0)
}

pub fn json_str(obj: &Value, key: &str) -> String {
    obj.get(key)
        .and_then(|v| {
            v.as_str()
                .map(str::to_string)
                .or_else(|| v.as_i64().map(|n| n.to_string()))
                .or_else(|| v.as_u64().map(|n| n.to_string()))
        })
        .unwrap_or_default()
}
