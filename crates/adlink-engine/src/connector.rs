//! Platform connector trait contract, onboarding guide descriptor, and retryable error classification.

use adlink_domain::{
    AdAccount, AdCreationRequest, AdCreationResult, AdCreationSupport, AdEntity, AdMutationRequest,
    AdMutationResult, DailyFundStatement, DiscoveredPlatformAccount, PlatformId, PlatformMeta,
    ReportGranularity, ReportLevel, TimeLine, UnifiedReportRow,
};
use async_trait::async_trait;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type ConnectorResult<T> = Result<T, ConnectorError>;

#[derive(Debug, Error)]
pub enum ConnectorError {
    #[error("rate limited by {platform}: retry after {retry_after_ms:?}ms ({message})")]
    RateLimited {
        platform: PlatformId,
        retry_after_ms: Option<u64>,
        message: String,
    },

    #[error("transient network/upstream error on {platform}: {message}")]
    Transient {
        platform: PlatformId,
        message: String,
    },

    #[error("authentication/authorization failed on {platform}: {message}")]
    AuthError {
        platform: PlatformId,
        message: String,
    },

    #[error("invalid request or unsupported operation on {platform}: {message}")]
    InvalidRequest {
        platform: PlatformId,
        message: String,
    },

    #[error("response parse error on {platform}: {message}")]
    ParseError {
        platform: PlatformId,
        message: String,
    },

    #[error("platform {platform} requires async task polling (task_id={task_id})")]
    AsyncTaskPending {
        platform: PlatformId,
        task_id: String,
    },
}

impl ConnectorError {
    /// Returns true when the error is safe to retry with exponential backoff.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::RateLimited { .. } | Self::Transient { .. })
    }

    pub fn retry_after_ms(&self) -> Option<u64> {
        match self {
            Self::RateLimited { retry_after_ms, .. } => *retry_after_ms,
            _ => None,
        }
    }
}

/// Request sent to an [`AdPlatformConnector`] for a single chunk & page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportFetchRequest {
    pub account: AdAccount,
    pub start_date: NaiveDate,
    pub end_date: NaiveDate,
    pub granularity: ReportGranularity,
    pub level: ReportLevel,
    pub time_line: TimeLine,
    pub page: u32,
    pub page_size: u32,
}

/// Paginated response returned by a connector after parsing official platform JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportFetchPage {
    pub rows: Vec<UnifiedReportRow>,
    pub has_more: bool,
    pub total_count: Option<u64>,
    pub next_cursor: Option<String>,
}

/// Credential field specification included in agent onboarding instructions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialFieldSpec {
    pub field: String,
    pub required: bool,
    pub secret: bool,
    pub description: String,
    pub example: String,
}

/// Structured step-by-step guide teaching a user's AI agent how to connect an official platform API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformOnboardGuide {
    pub platform: PlatformId,
    pub mode: String,
    pub developer_portal_url: String,
    pub oauth_or_auth_flow: String,
    pub optimal_login_strategy: String,
    pub fallback_login_strategy: String,
    pub ad_creation_support: AdCreationSupport,
    pub ad_creation_ask_prompt: String,
    pub ad_creation_risks: Vec<String>,
    pub required_permissions: Vec<String>,
    pub prerequisite_steps: Vec<String>,
    pub credential_fields: Vec<CredentialFieldSpec>,
    pub toml_snippet: String,
    pub verification_steps: Vec<String>,
    pub gotchas: Vec<String>,
}

/// Unified connector contract implemented by every ad platform plugin and registered in `Kernel`.
#[async_trait]
pub trait AdPlatformConnector: Send + Sync {
    /// Platform enum identifier.
    fn platform_id(&self) -> PlatformId;

    /// Official platform metadata & canonical field mapping table.
    fn meta(&self) -> PlatformMeta {
        self.platform_id().meta()
    }

    /// Returns whether this connector is running in `mock` or `live` mode.
    fn mode_label(&self) -> &'static str;

    /// Detailed official API connection walkthrough tailored for human operators & AI agents.
    fn onboard_guide(&self) -> PlatformOnboardGuide;

    /// Discovers authorized advertiser/sub-accounts using Tier-1 Official MCP Bridge (optimal)
    /// or Tier-2 Direct API Account List (fallback).
    async fn discover_accounts(&self) -> ConnectorResult<Vec<DiscoveredPlatformAccount>> {
        Ok(Vec::new())
    }

    /// Fetches one page of normalized report rows (already parsed from official platform JSON).
    async fn fetch_reports_page(&self, req: &ReportFetchRequest) -> ConnectorResult<ReportFetchPage>;

    /// Fetches daily cash/grant fund statements for financial reconciliation.
    async fn fetch_fund_statements(
        &self,
        account: &AdAccount,
        start_date: NaiveDate,
        end_date: NaiveDate,
    ) -> ConnectorResult<Vec<DailyFundStatement>>;

    /// Lists campaigns/adgroups/units for operational inspection or mutation targeting.
    async fn list_ad_entities(
        &self,
        account: &AdAccount,
        level: ReportLevel,
    ) -> ConnectorResult<Vec<AdEntity>>;

    /// Applies or simulates (`dry_run = true`) an operational mutation.
    async fn mutate_ad_entities(
        &self,
        account: &AdAccount,
        request: &AdMutationRequest,
    ) -> ConnectorResult<AdMutationResult>;

    /// Opt-in programmatic ad creation (`创编`: campaign / ad_group / creative).
    /// Disabled by default by `AdSafetyGuardrailPlugin` until user consent + risk acknowledgment.
    async fn create_ad_entity(
        &self,
        _account: &AdAccount,
        request: &AdCreationRequest,
    ) -> ConnectorResult<AdCreationResult> {
        Err(ConnectorError::InvalidRequest {
            platform: self.platform_id(),
            message: format!(
                "platform '{}' ({:?}) does not expose a public self-serve ad creation API for {:?}",
                self.platform_id().display_name_zh(),
                self.meta().ad_creation_support,
                request.entity_kind
            ),
        })
    }
}
