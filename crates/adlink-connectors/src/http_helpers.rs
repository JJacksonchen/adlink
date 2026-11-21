//! Shared HTTP client configuration, credential config types, JSON helpers,
//! Official-MCP auth bridge discovery, and official response error mapping.

use std::collections::BTreeMap;
use std::time::Duration;

use adlink_domain::{
    AccountType, AdCreationRiskDisclosure, AdCreationSupport, AuthLoginStrategy,
    DiscoveredPlatformAccount, Fen, PlatformId,
};
use adlink_engine::ConnectorError;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::mock_data::mock_platform_account_id;

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
    /// Preferred login/discovery strategy (`official_mcp_bridge` vs `direct_api_account_discovery`).
    #[serde(default)]
    pub auth_strategy: AuthLoginStrategy,
    /// Optional URL of the platform's Official MCP server for zero-OAuth login/account reuse (Tier 1).
    #[serde(default)]
    pub official_mcp_endpoint: Option<String>,
    /// MCP tool name invoked on `official_mcp_endpoint` to pull authorized accounts (default `"list_accounts"`).
    #[serde(default)]
    pub official_mcp_auth_tool: Option<String>,
    /// Optional Bearer token for the official MCP server (falls back to `access_token`).
    #[serde(default)]
    pub official_mcp_token: Option<String>,
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
    /// Optional extra headers/parameters required by agency gateways or platform flags.
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
                .or_else(|| {
                    v.as_str()
                        .and_then(|s| s.parse::<f64>().ok().map(|f| f.max(0.0).round() as u64))
                })
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

/// Extracts a non-empty string or numeric ID (`campaign_id`, `material_id`, `poi_id`, etc.) as `Option<String>`.
pub fn json_opt_str(obj: &Value, key: &str) -> Option<String> {
    let s = json_str(obj, key);
    if s.trim().is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Parses a monetary value into exact [`Fen`], handling integer fen OR float/decimal-string yuan cleanly.
///
/// Useful for endpoints such as Xiaohongshu Juguang/Chengfeng where whitepaper versions or gateways
/// may return `"123.45"` (yuan) vs `12345` (fen).
pub fn json_money_fen(obj: &Value, key: &str, default_unit_yuan: bool) -> Fen {
    let Some(val) = obj.get(key) else {
        return Fen::ZERO;
    };
    if let Some(s) = val.as_str() {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Fen::ZERO;
        }
        if trimmed.contains('.') || default_unit_yuan {
            return Fen::from_yuan_f64(trimmed.parse::<f64>().unwrap_or(0.0));
        }
        return Fen::from_fen(trimmed.parse::<i64>().unwrap_or(0));
    }
    if let Some(n) = val.as_i64() {
        return if default_unit_yuan {
            Fen::from_yuan_f64(n as f64)
        } else {
            Fen::from_fen(n)
        };
    }
    if let Some(u) = val.as_u64() {
        return if default_unit_yuan {
            Fen::from_yuan_f64(u as f64)
        } else {
            Fen::from_fen(u as i64)
        };
    }
    if let Some(f) = val.as_f64() {
        if default_unit_yuan || f.fract().abs() > f64::EPSILON {
            return Fen::from_yuan_f64(f);
        }
        return Fen::from_fen(f.round() as i64);
    }
    Fen::ZERO
}

/// Merges optional `platform_extra` JSON object fields into a platform request payload object.
pub fn merge_json_extra(payload: &mut Value, extra: &Value) {
    if let (Some(target), Some(src)) = (payload.as_object_mut(), extra.as_object()) {
        for (k, v) in src {
            target.insert(k.clone(), v.clone());
        }
    }
}

/// Builds deterministic ad creation risk warnings tailored for the platform.
pub fn platform_creation_notes(platform: PlatformId, extra_note: Option<&str>) -> Vec<String> {
    let meta = platform.meta();
    let mut notes = vec![format!(
        "Creation support for {}: {:?} (endpoints: {})",
        meta.name_zh,
        meta.ad_creation_support,
        if meta.ad_creation_endpoints.is_empty() {
            "none".into()
        } else {
            meta.ad_creation_endpoints.join(", ")
        }
    )];
    if meta.ad_creation_support == AdCreationSupport::OfficialPartnerApi {
        notes.push(
            "NOTE: Official Partner API creation requires enterprise/agency whitelist approval on the media platform."
                .into(),
        );
    }
    if let Some(extra) = extra_note {
        notes.push(extra.to_string());
    }
    notes.push(AdCreationRiskDisclosure::standard_risk_warnings()[0].clone());
    notes
}

/// Tier-1 Optimal Auth Strategy: calls the platform's Official MCP server (Streamable HTTP `tools/call`)
/// to list accounts already authenticated inside the official MCP session, eliminating duplicate OAuth.
///
/// Returns `None` when `official_mcp_endpoint` is not configured or unreachable, signaling the
/// caller to fall back cleanly to Tier-2 Direct Marketing API account list discovery.
pub async fn discover_via_official_mcp_bridge(
    client: &Client,
    platform: PlatformId,
    creds: &LiveCredentials,
) -> Option<Vec<DiscoveredPlatformAccount>> {
    let endpoint = creds
        .official_mcp_endpoint
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let tool_name = creds
        .official_mcp_auth_tool
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("list_accounts");
    let token = creds
        .official_mcp_token
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(creds.access_token.as_str());

    let mut req = client.post(endpoint).json(&json!({
        "jsonrpc": "2.0",
        "id": "adlink-auth-bridge",
        "method": "tools/call",
        "params": {
            "name": tool_name,
            "arguments": { "platform": platform.as_str() }
        }
    }));
    if !token.trim().is_empty() {
        req = req.bearer_auth(token);
    }
    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: Value = resp.json().await.ok()?;
    let result = body.get("result").unwrap_or(&body);
    let structured = result
        .get("structuredContent")
        .or_else(|| result.get("data"))
        .unwrap_or(result);

    let owned_list: Vec<Value> = if let Some(arr) = structured
        .get("accounts")
        .or_else(|| structured.get("advertisers"))
        .or_else(|| structured.get("list"))
        .and_then(Value::as_array)
    {
        arr.clone()
    } else if let Some(txt) = result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|c| c.get("text"))
        .and_then(Value::as_str)
    {
        let parsed: Value = serde_json::from_str(txt).ok()?;
        parsed
            .get("accounts")
            .or_else(|| parsed.get("advertisers"))
            .or_else(|| parsed.get("list"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    } else {
        return None;
    };

    if owned_list.is_empty() {
        return None;
    }

    Some(
        owned_list
            .into_iter()
            .map(|item| {
                let pid = json_str(&item, "platform_account_id")
                    .if_empty_then(|| json_str(&item, "account_id"))
                    .if_empty_then(|| json_str(&item, "advertiser_id"));
                let name = json_str(&item, "account_name")
                    .if_empty_then(|| json_str(&item, "advertiser_name"))
                    .if_empty_then(|| format!("{} Account {}", platform.display_name_zh(), pid));
                DiscoveredPlatformAccount {
                    platform,
                    platform_account_id: pid,
                    account_name: name,
                    account_type: AccountType::AgencySub,
                    status: json_str(&item, "status").if_empty_then(|| "active".into()),
                    parent_agent_id: json_opt_str(&item, "parent_agent_id"),
                    auth_strategy: AuthLoginStrategy::OfficialMcpBridge,
                    already_registered_ad_account_id: None,
                    current_person_ids: Vec::new(),
                    current_project_ids: Vec::new(),
                    raw_extra: item,
                }
            })
            .filter(|a| !a.platform_account_id.is_empty())
            .collect(),
    )
}

/// Generates realistic discovered platform sub-accounts in Mock mode (both primary seeded account
/// and additional newly authorized sub-accounts ready to be relabeled or bootstrapped into projects).
pub fn mock_discovered_accounts(
    platform: PlatformId,
    strategy: AuthLoginStrategy,
) -> Vec<DiscoveredPlatformAccount> {
    let primary_id = mock_platform_account_id(platform).to_string();
    let sub_a = format!("{primary_id}01");
    let sub_b = format!("{primary_id}02");
    vec![
        DiscoveredPlatformAccount {
            platform,
            platform_account_id: primary_id,
            account_name: format!("{} - 主授权代理子户", platform.display_name_zh()),
            account_type: AccountType::AgencySub,
            status: "active".into(),
            parent_agent_id: Some(format!("agent-{}", platform.as_str())),
            auth_strategy: strategy,
            already_registered_ad_account_id: None,
            current_person_ids: Vec::new(),
            current_project_ids: Vec::new(),
            raw_extra: json!({ "source": "mock_discovery", "primary": true }),
        },
        DiscoveredPlatformAccount {
            platform,
            platform_account_id: sub_a,
            account_name: format!("{} - 新发现护肤线子户A", platform.display_name_zh()),
            account_type: AccountType::AgencySub,
            status: "active".into(),
            parent_agent_id: Some(format!("agent-{}", platform.as_str())),
            auth_strategy: strategy,
            already_registered_ad_account_id: None,
            current_person_ids: Vec::new(),
            current_project_ids: Vec::new(),
            raw_extra: json!({ "source": "mock_discovery", "primary": false }),
        },
        DiscoveredPlatformAccount {
            platform,
            platform_account_id: sub_b,
            account_name: format!("{} - 新发现本地/达人扩展子户B", platform.display_name_zh()),
            account_type: AccountType::BrandDirect,
            status: "active".into(),
            parent_agent_id: Some(format!("agent-{}", platform.as_str())),
            auth_strategy: strategy,
            already_registered_ad_account_id: None,
            current_person_ids: Vec::new(),
            current_project_ids: Vec::new(),
            raw_extra: json!({ "source": "mock_discovery", "primary": false }),
        },
    ]
}

trait StrFallback {
    fn if_empty_then(self, f: impl FnOnce() -> String) -> String;
}

impl StrFallback for String {
    fn if_empty_then(self, f: impl FnOnce() -> String) -> String {
        if self.trim().is_empty() {
            f()
        } else {
            self
        }
    }
}
