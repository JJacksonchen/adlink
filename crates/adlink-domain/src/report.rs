//! Canonical unified report rows, daily fund statements, and sync job tracking structures.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::money::Fen;
use crate::platform::PlatformId;

/// Temporal aggregation granularity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportGranularity {
    Daily,
    Hourly,
}

impl ReportGranularity {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Hourly => "hourly",
        }
    }
}

/// Advertising entity hierarchy level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportLevel {
    Account,
    Campaign,
    AdGroup,
    Creative,
    ContentOrder,
}

impl ReportLevel {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Account => "account",
            Self::Campaign => "campaign",
            Self::AdGroup => "ad_group",
            Self::Creative => "creative",
            Self::ContentOrder => "content_order",
        }
    }
}

/// Attribution timeline matching Tencent AMS v3.0 `REPORTING_TIME` vs `REQUEST_TIME`
/// and Alipay `conv_time_join_rule`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeLine {
    #[default]
    ReportingTime,
    RequestTime,
}

impl TimeLine {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ReportingTime => "reporting_time",
            Self::RequestTime => "request_time",
        }
    }
}

/// Canonical unified report row stored with zero loss of official upstream fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedReportRow {
    pub platform: PlatformId,
    pub ad_account_id: String,
    pub platform_account_id: String,
    pub stat_date: NaiveDate,
    /// `0..=23` for hourly reports, `-1` for daily reports.
    pub stat_hour: i32,
    pub granularity: ReportGranularity,
    pub level: ReportLevel,
    pub entity_id: String,
    pub entity_name: String,
    pub parent_entity_id: Option<String>,
    /// Shared cross-platform content identifier (`note_id`, `bvid`, `photo_id`, `material_id`).
    pub content_id: Option<String>,
    pub time_line: TimeLine,
    pub gross_cost_fen: Fen,
    pub cash_cost_fen: Fen,
    pub grant_cost_fen: Fen,
    pub kol_order_fee_fen: Fen,
    pub platform_service_fee_fen: Fen,
    pub impressions: u64,
    pub clicks: u64,
    pub conversions: u64,
    pub deep_conversions: u64,
    pub leads: u64,
    pub orders: u64,
    pub gmv_fen: Fen,
    pub interactions: u64,
    pub video_plays: u64,
    pub effective_plays: u64,
    pub new_follows: u64,
    pub store_verifies: u64,
    /// Preserves every raw official JSON metric/dimension verbatim.
    #[serde(default)]
    pub raw_fields: BTreeMap<String, Value>,
}

impl Default for UnifiedReportRow {
    fn default() -> Self {
        Self {
            platform: PlatformId::TencentAds,
            ad_account_id: String::new(),
            platform_account_id: String::new(),
            stat_date: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap_or_default(),
            stat_hour: -1,
            granularity: ReportGranularity::Daily,
            level: ReportLevel::AdGroup,
            entity_id: String::new(),
            entity_name: String::new(),
            parent_entity_id: None,
            content_id: None,
            time_line: TimeLine::ReportingTime,
            gross_cost_fen: Fen::ZERO,
            cash_cost_fen: Fen::ZERO,
            grant_cost_fen: Fen::ZERO,
            kol_order_fee_fen: Fen::ZERO,
            platform_service_fee_fen: Fen::ZERO,
            impressions: 0,
            clicks: 0,
            conversions: 0,
            deep_conversions: 0,
            leads: 0,
            orders: 0,
            gmv_fen: Fen::ZERO,
            interactions: 0,
            video_plays: 0,
            effective_plays: 0,
            new_follows: 0,
            store_verifies: 0,
            raw_fields: BTreeMap::new(),
        }
    }
}

/// Daily wallet / cash / grant reconciliation record per ad account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyFundStatement {
    pub platform: PlatformId,
    pub ad_account_id: String,
    pub platform_account_id: String,
    pub stat_date: NaiveDate,
    pub cash_balance_fen: Fen,
    pub grant_balance_fen: Fen,
    pub cash_cost_fen: Fen,
    pub grant_cost_fen: Fen,
    pub shared_wallet_cost_fen: Fen,
    pub income_fen: Fen,
    #[serde(default)]
    pub raw_fields: BTreeMap<String, Value>,
}

/// Query filters used by both REST analytics endpoints and MCP query tools.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReportQuery {
    pub start_date: NaiveDate,
    pub end_date: NaiveDate,
    #[serde(default)]
    pub platforms: Vec<PlatformId>,
    #[serde(default)]
    pub ad_account_ids: Vec<String>,
    #[serde(default)]
    pub project_ids: Vec<String>,
    #[serde(default)]
    pub person_ids: Vec<String>,
    #[serde(default)]
    pub levels: Vec<ReportLevel>,
    #[serde(default)]
    pub group_by: Vec<QueryDimension>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryDimension {
    Date,
    Platform,
    Account,
    Project,
    ProjectType,
    Person,
    Entity,
    Content,
}

/// Persisted batch sync job record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncJobRecord {
    pub id: String,
    pub status: String,
    pub platforms: Vec<PlatformId>,
    pub start_date: NaiveDate,
    pub end_date: NaiveDate,
    pub total_chunks: usize,
    pub completed_chunks: usize,
    pub failed_chunks: usize,
    pub rows_upserted: usize,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error_summary: Option<String>,
}

/// Individual chunk checkpoint enabling idempotent resume across crashes or rate-limit interruptions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncCheckpoint {
    pub job_id: String,
    pub platform: PlatformId,
    pub ad_account_id: String,
    pub level: ReportLevel,
    pub chunk_start: NaiveDate,
    pub chunk_end: NaiveDate,
    pub status: String,
    pub rows_fetched: usize,
    pub error_message: Option<String>,
    pub updated_at: DateTime<Utc>,
}
