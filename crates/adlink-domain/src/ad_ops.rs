//! Advertising campaign/ad-group entity state, safe mutation requests, and audit log entries.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::money::Fen;
use crate::platform::PlatformId;
use crate::report::ReportLevel;

/// Cached or live campaign/unit/ad-group entity managed across platforms.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdEntity {
    pub platform: PlatformId,
    pub ad_account_id: String,
    pub level: ReportLevel,
    pub entity_id: String,
    pub entity_name: String,
    pub parent_entity_id: Option<String>,
    pub status: String,
    pub daily_budget_fen: Fen,
    pub bid_fen: Fen,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub raw_extra: Value,
}

/// Supported write operations on upstream ad entities.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AdMutationAction {
    Pause,
    Resume,
    UpdateBudget { daily_budget_fen: i64 },
    UpdateBid { bid_fen: i64 },
}

/// Safety-checked mutation command passed through Kernel `BeforeAdMutate` / `AfterAdMutate` hooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdMutationRequest {
    pub platform: PlatformId,
    pub ad_account_id: String,
    pub entity_level: ReportLevel,
    pub entity_ids: Vec<String>,
    pub action: AdMutationAction,
    #[serde(default = "default_true")]
    pub dry_run: bool,
    pub reason: String,
    #[serde(default = "default_operator")]
    pub operator: String,
}

fn default_true() -> bool {
    true
}

fn default_operator() -> String {
    "mcp-agent".into()
}

/// Structured result of executing or dry-running an [`AdMutationRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdMutationResult {
    pub dry_run: bool,
    pub accepted: bool,
    pub platform: PlatformId,
    pub ad_account_id: String,
    pub affected_entity_ids: Vec<String>,
    pub summary: String,
    pub guardrail_notes: Vec<String>,
    #[serde(default)]
    pub platform_response: Value,
}

/// Immutable audit record written whenever an ad mutation or guardrail rejection occurs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub operator: String,
    pub source: String,
    pub platform: PlatformId,
    pub ad_account_id: String,
    pub action: String,
    pub entity_ids: Vec<String>,
    pub dry_run: bool,
    pub status: String,
    pub reason: String,
    pub detail: Value,
}
