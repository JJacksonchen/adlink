//! Advertising campaign/ad-group entity state, safe mutation & opt-in creation (`创编`) models,
//! and immutable audit log entries.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::money::Fen;
use crate::platform::{AdCreationSupport, PlatformId};
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

fn default_paused_status() -> String {
    "PAUSED".into()
}

fn default_max_create_budget() -> i64 {
    10_000_000 // ¥100,000.00/day
}

fn default_max_create_bid() -> i64 {
    300_000 // ¥3,000.00
}

/// Opt-in configuration governing programmatic Ad Creation (`创编`: creating campaigns/adgroups/creatives).
///
/// SECURITY DEFAULT: Disabled (`enabled = false`, `risk_acknowledged = false`).
/// Operators and AI agents MUST ask the user and disclose creation risks before enabling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdCreationConfig {
    /// Must be explicitly set to `true` after user consent. Defaults to `false`.
    #[serde(default)]
    pub enabled: bool,
    /// Must be explicitly set to `true` confirming the user acknowledged ad-creation financial & policy risks.
    #[serde(default)]
    pub risk_acknowledged: bool,
    /// Requires a `dry_run = true` preview before `dry_run = false` live submission (`preview_confirmed = true`).
    #[serde(default = "default_true")]
    pub require_dry_run_preview: bool,
    /// Hard initial daily budget ceiling in Fen (default: ¥100,000 = 10_000_000 fen).
    #[serde(default = "default_max_create_budget")]
    pub max_initial_daily_budget_fen: i64,
    /// Hard initial bid ceiling in Fen (default: ¥3,000 = 300_000 fen).
    #[serde(default = "default_max_create_bid")]
    pub max_initial_bid_fen: i64,
    /// Optional allowlist of platforms permitted for ad creation (empty = all creation-capable platforms).
    #[serde(default)]
    pub allowed_platforms: Vec<PlatformId>,
}

impl Default for AdCreationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            risk_acknowledged: false,
            require_dry_run_preview: true,
            max_initial_daily_budget_fen: default_max_create_budget(),
            max_initial_bid_fen: default_max_create_bid(),
            allowed_platforms: Vec::new(),
        }
    }
}

/// Target hierarchy entity kind when creating an ad structure via API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdCreationEntityKind {
    Campaign,
    AdGroup,
    Creative,
}

impl AdCreationEntityKind {
    pub const fn to_report_level(&self) -> ReportLevel {
        match self {
            Self::Campaign => ReportLevel::Campaign,
            Self::AdGroup => ReportLevel::AdGroup,
            Self::Creative => ReportLevel::Creative,
        }
    }
}

/// Guarded programmatic ad creation request routed through `BeforeAdCreate` / `AfterAdCreate` hooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdCreationRequest {
    pub platform: PlatformId,
    pub ad_account_id: String,
    pub entity_kind: AdCreationEntityKind,
    pub name: String,
    pub parent_entity_id: Option<String>,
    pub marketing_goal: Option<String>,
    pub daily_budget_fen: Option<i64>,
    pub bid_fen: Option<i64>,
    #[serde(default)]
    pub targeting: Value,
    #[serde(default)]
    pub creative_assets: Value,
    pub landing_url: Option<String>,
    /// Defaults to `"PAUSED"` so newly created ads never burn live budget until reviewed.
    #[serde(default = "default_paused_status")]
    pub initial_status: String,
    /// Defaults to `true`; generates official payload preview without writing upstream.
    #[serde(default = "default_true")]
    pub dry_run: bool,
    /// Must be `true` when submitting `dry_run = false` if `require_dry_run_preview = true`.
    #[serde(default)]
    pub preview_confirmed: bool,
    pub reason: String,
    #[serde(default = "default_operator")]
    pub operator: String,
    /// Platform-specific raw fields merged into the official upstream creation JSON payload.
    #[serde(default)]
    pub platform_extra: Value,
}

/// Structured result from previewing or submitting an [`AdCreationRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdCreationResult {
    pub dry_run: bool,
    pub accepted: bool,
    pub platform: PlatformId,
    pub ad_account_id: String,
    pub entity_kind: AdCreationEntityKind,
    pub created_entity_id: Option<String>,
    pub initial_status: String,
    pub summary: String,
    pub risk_warnings: Vec<String>,
    pub guardrail_notes: Vec<String>,
    #[serde(default)]
    pub platform_payload_preview: Value,
    #[serde(default)]
    pub platform_response: Value,
}

/// Per-platform ad creation support summary shown in risk disclosure payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformCreationCapability {
    pub platform: PlatformId,
    pub name_zh: String,
    pub support: AdCreationSupport,
    pub endpoints: Vec<String>,
}

/// Complete opt-in status, mandatory user prompt, risk checklist, and platform support matrix for `创编`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdCreationRiskDisclosure {
    pub enabled: bool,
    pub risk_acknowledged: bool,
    pub ready_for_creation: bool,
    pub require_dry_run_preview: bool,
    pub max_initial_daily_budget_fen: i64,
    pub max_initial_bid_fen: i64,
    pub default_ask_user_prompt: String,
    pub risk_warnings: Vec<String>,
    pub how_to_enable_toml: String,
    pub platform_capabilities: Vec<PlatformCreationCapability>,
}

impl AdCreationRiskDisclosure {
    pub fn standard_risk_warnings() -> Vec<String> {
        vec![
            "【资金与误烧预算风险】创编接口会直接在媒体账户生成可消耗的计划/单元/创意；部分媒体单位不同（快手使用「厘」、腾讯/小红书使用「分」、巨量使用「元」），哪怕一次参数或小数点误填均可能导致预算瞬时高额消耗。".into(),
            "【审核拒审与账户封禁风险】AI 批量生成的标题、图片、视频或落地页若触犯平台广告法/行业准入规则（医疗、美妆功效宣称等），可能引发审核拒审、主体信用分扣除乃至封户。".into(),
            "【定向与归因错配风险】错误的地域/人群定向、错误的落地页 URL 或转化跟踪像素 ID，会导致大量无效点击且无法回传转化归因。".into(),
            "【平台接口与白名单差异】腾讯 AMS v3.0 已合并计划为「广告组+动态创意」两层；小红书聚光/乘风与B站三连需官方合作白名单权限；支付宝灯火及蒲公英/花火无公开自助创编 API。".into(),
            "【默认安全护栏规则】AdLink 默认关闭创编（enabled=false），即使开启也强制：1) 默认 initial_status=PAUSED（暂停态创建）；2) 默认 dry_run=true 先输出平台原始报文预览；3) 日预算与出价受硬上限保护并全量写入不可变审计日志。".into(),
        ]
    }

    pub fn from_config(cfg: &AdCreationConfig) -> Self {
        let caps = PlatformId::ALL
            .iter()
            .map(|p| {
                let meta = p.meta();
                PlatformCreationCapability {
                    platform: *p,
                    name_zh: meta.name_zh.to_string(),
                    support: meta.ad_creation_support,
                    endpoints: meta
                        .ad_creation_endpoints
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                }
            })
            .collect();

        Self {
            enabled: cfg.enabled,
            risk_acknowledged: cfg.risk_acknowledged,
            ready_for_creation: cfg.enabled && cfg.risk_acknowledged,
            require_dry_run_preview: cfg.require_dry_run_preview,
            max_initial_daily_budget_fen: cfg.max_initial_daily_budget_fen,
            max_initial_bid_fen: cfg.max_initial_bid_fen,
            default_ask_user_prompt: "当前 AdLink 默认处于【仅只读拉数 + Dry-Run 运维】安全模式，广告创编（新建推广计划/单元/创意）默认关闭。接入前请先询问用户：『是否需要接入广告创编（新建计划/单元/创意）能力？』，并完整展示以下 5 项创编风险；在用户明确确认并完成配置前，严禁擅自开启或调用写接口。".into(),
            risk_warnings: Self::standard_risk_warnings(),
            how_to_enable_toml: "[ad_creation]\nenabled = true\nrisk_acknowledged = true\nrequire_dry_run_preview = true\nmax_initial_daily_budget_fen = 10000000\nmax_initial_bid_fen = 300000".into(),
            platform_capabilities: caps,
        }
    }
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

/// Immutable audit record written whenever an ad mutation, ad creation, governance relabel,
/// or guardrail rejection occurs.
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
