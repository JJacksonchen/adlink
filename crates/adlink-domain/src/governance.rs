//! Organization, People, Ad Accounts, Person-Account (`人-户关系`), and Project classification models.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::money::Fen;
use crate::platform::PlatformId;

/// Enterprise tenant or agency organization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Organization {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub org_type: OrgType,
    pub default_currency: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrgType {
    Advertiser,
    Agency,
    HybridGroup,
}

/// Team role within advertiser or agency operations.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonRole {
    Optimizer,
    MediaBuyer,
    AccountManager,
    Sales,
    CreativeDirector,
    Copywriter,
    Videographer,
    Editor,
    Designer,
    Talent,
    FdeAdmin,
    Custom(String),
}

/// Internal employee, creator, or operator participating in ad execution or production.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Person {
    pub id: String,
    pub org_id: String,
    pub name: String,
    pub email: Option<String>,
    pub department: String,
    pub role: PersonRole,
    pub monthly_cost_fen: Fen,
    pub active: bool,
}

/// Managed ad account across any of the supported platforms.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdAccount {
    pub id: String,
    pub org_id: String,
    pub platform: PlatformId,
    pub platform_account_id: String,
    pub account_name: String,
    pub account_type: AccountType,
    /// Media rebate ratio (e.g. `0.06` means 6% rebate back on media spend).
    pub rebate_rate: f64,
    /// Agency service fee ratio charged to client (e.g. `0.08` means 8%).
    pub service_fee_rate: f64,
    /// Fallback cash/grant split when daily fund statement is not yet synced (e.g. `0.85`).
    pub default_cash_ratio: f64,
    pub status: String,
    /// Logical key referencing credentials in `config` (never stores raw secret tokens in DB).
    pub credential_ref: Option<String>,
    #[serde(default)]
    pub extra: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountType {
    SelfServe,
    AgencySub,
    BrandDirect,
    CreatorWallet,
}

/// Time-bounded `人-户关系` binding assigning operational/performance ownership of an account
/// to a person with a rational split weight (e.g. `0.6` / `0.4` co-optimization or mid-month handoff).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonAccountBinding {
    pub id: String,
    pub person_id: String,
    pub ad_account_id: String,
    pub relation_role: BindingRole,
    /// Share of attributed spend/performance assigned to this person (`0.0..=1.0`).
    pub split_weight: f64,
    pub effective_from: NaiveDate,
    pub effective_to: Option<NaiveDate>,
}

impl PersonAccountBinding {
    /// Returns true if this binding covers `date` inclusively.
    pub fn is_active_on(&self, date: NaiveDate) -> bool {
        date >= self.effective_from && self.effective_to.map_or(true, |end| date <= end)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingRole {
    PrimaryOptimizer,
    CoOptimizer,
    AccountManager,
    SalesOwner,
    MediaBuyer,
}

/// High-level business classification of marketing projects (`项目类型分类`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectType {
    /// 效果获客 / 线索表单 / App下载激活
    PerformanceAcquisition,
    /// 电商成交 / 闭环GMV / ROI考核
    EcommerceGmv,
    /// 内容种草 / 蒲公英+聚光 / 花火+起飞
    SeedingContent,
    /// 本地生活 / 团购到店核销
    LocalLife,
    /// 品牌曝光 / 心智触达
    BrandAwareness,
    /// 直播带货 / 直播引流
    LiveStreaming,
    Custom(String),
}

/// Client or brand campaign project grouping multiple platform accounts & creative assets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub org_id: String,
    pub code: String,
    pub name: String,
    pub client_name: String,
    pub brand_name: String,
    pub project_type: ProjectType,
    pub business_line: String,
    pub settlement_mode: SettlementMode,
    pub budget_fen: Fen,
    pub target_roi: Option<f64>,
    pub target_cpa_fen: Option<Fen>,
    pub owner_person_id: Option<String>,
    pub start_date: NaiveDate,
    pub end_date: Option<NaiveDate>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettlementMode {
    /// 消耗返点 + 服务费结算
    MediaSpendPlusFee,
    /// 纯效果 CPA/CPS 结算
    PerformanceCpaCps,
    /// 固定月费 / 全案 Retainer
    FixedRetainer,
    /// 品牌采买 + 达人商单打包
    IntegratedPackage,
}

/// Maps a project to an ad account, optionally scoping down via campaign name regex.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectAccountLink {
    pub id: String,
    pub project_id: String,
    pub ad_account_id: String,
    pub campaign_filter_regex: Option<String>,
    pub allocation_weight: f64,
}
