//! Two-tier platform login/authentication bridge & internal governance re-labeling models.
//!
//! - **Optimal Strategy (`OfficialMcpBridge`)**: Reuse the official ad platform's MCP login/auth
//!   endpoint to authenticate and pull advertiser accounts, then freely **re-label** internal
//!   `人-户关系` (`PersonAccountBinding`) and `项目关系` (`ProjectAccountLink`) inside AdLink.
//! - **Fallback Strategy (`DirectApiAccountDiscovery`)**: When a platform MCP is unavailable or
//!   headless credentials are preferred, pull the advertiser/sub-account list via Marketing API
//!   and bootstrap custom projects linked directly to selected sub-accounts.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::governance::{AccountType, BindingRole, Project};
use crate::platform::PlatformId;

fn default_one() -> f64 {
    1.0
}

fn default_binding_role() -> BindingRole {
    BindingRole::PrimaryOptimizer
}

/// How an ad account obtained its platform authentication and sub-account list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AuthLoginStrategy {
    /// Optimal: Authenticate via official media platform MCP login/auth interface, then
    /// re-label `人-户关系` and `项目关系` cleanly inside AdLink.
    #[default]
    OfficialMcpBridge,
    /// Fallback: Directly call official Marketing API to pull advertiser/sub-account lists,
    /// then create custom projects and link sub-accounts in AdLink.
    DirectApiAccountDiscovery,
}

/// Account record discovered from either Official Platform MCP Bridge (optimal) or Direct API (fallback),
/// enriched with current AdLink internal governance mappings when already registered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredPlatformAccount {
    pub platform: PlatformId,
    pub platform_account_id: String,
    pub account_name: String,
    pub account_type: AccountType,
    pub status: String,
    pub parent_agent_id: Option<String>,
    pub auth_strategy: AuthLoginStrategy,
    pub already_registered_ad_account_id: Option<String>,
    #[serde(default)]
    pub current_person_ids: Vec<String>,
    #[serde(default)]
    pub current_project_ids: Vec<String>,
    #[serde(default)]
    pub raw_extra: Value,
}

/// Single `人-户关系` binding entry submitted during governance re-labeling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelabelPersonBindingInput {
    pub person_id: String,
    #[serde(default = "default_binding_role")]
    pub relation_role: BindingRole,
    #[serde(default = "default_one")]
    pub split_weight: f64,
    pub effective_from: NaiveDate,
    pub effective_to: Option<NaiveDate>,
}

/// Single `项目-户关系` link entry submitted during governance re-labeling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelabelProjectLinkInput {
    pub project_id: String,
    pub campaign_filter_regex: Option<String>,
    #[serde(default = "default_one")]
    pub allocation_weight: f64,
}

/// Request to register/update an authenticated platform account and **re-label (`重标`)**
/// its enterprise `人-户关系` & `项目关系` inside AdLink (Optimal Tier-1 workflow).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelabelAccountGovernanceRequest {
    pub platform: PlatformId,
    pub platform_account_id: String,
    pub ad_account_id: Option<String>,
    pub account_name: Option<String>,
    pub org_id: Option<String>,
    pub account_type: Option<AccountType>,
    #[serde(default)]
    pub auth_strategy: AuthLoginStrategy,
    pub credential_ref: Option<String>,
    pub official_mcp_endpoint: Option<String>,
    pub rebate_rate: Option<f64>,
    pub service_fee_rate: Option<f64>,
    pub default_cash_ratio: Option<f64>,
    #[serde(default)]
    pub replace_existing_bindings: bool,
    #[serde(default)]
    pub person_bindings: Vec<RelabelPersonBindingInput>,
    #[serde(default)]
    pub replace_existing_project_links: bool,
    #[serde(default)]
    pub project_links: Vec<RelabelProjectLinkInput>,
}

/// Sub-account binding specification used by [`FallbackBootstrapProjectRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackSubAccountBind {
    pub platform: PlatformId,
    pub platform_account_id: String,
    pub account_name: String,
    pub account_type: Option<AccountType>,
    pub rebate_rate: Option<f64>,
    pub service_fee_rate: Option<f64>,
    pub default_cash_ratio: Option<f64>,
    pub campaign_filter_regex: Option<String>,
    pub allocation_weight: Option<f64>,
    pub owner_person_id: Option<String>,
    pub relation_role: Option<BindingRole>,
    pub split_weight: Option<f64>,
    pub effective_from: Option<NaiveDate>,
}

/// Tier-2 fallback bootstrap request: create/update a custom project and bind pulled
/// advertiser sub-accounts (+ optional owner bindings) in one atomic governance step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackBootstrapProjectRequest {
    pub project: Project,
    pub accounts: Vec<FallbackSubAccountBind>,
    #[serde(default)]
    pub replace_existing_links: bool,
}
