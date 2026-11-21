//! Creative taxonomy (`创意类型分类`), cross-platform content linking (`note_id`/`bvid`),
//! and internal production workload attribution (`创作工作量与投放数据归因打通`).

use std::collections::BTreeMap;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::money::Fen;
use crate::platform::PlatformId;

/// Format category of an ad creative or seeded content piece.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreativeFormat {
    ShortVideo,
    LiveClip,
    ImageTextNote,
    Carousel,
    Splash,
    Playable,
    KolCustom,
    Custom(String),
}

/// Persuasion / opening hook classification for creative intelligence.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreativeHookType {
    PainPoint,
    Contrast,
    Unboxing,
    Story,
    PromoPrice,
    ExpertAuthority,
    Lifestyle,
    Custom(String),
}

/// Extensible creative taxonomy metadata (`创意类型分类`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreativeTaxonomy {
    pub format: CreativeFormat,
    pub hook_type: CreativeHookType,
    pub scene_tag: String,
    pub selling_point: String,
    pub tone: String,
    #[serde(default)]
    pub custom_tags: BTreeMap<String, String>,
}

/// Canonical internal creative asset spanning one or more ad platforms.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreativeAsset {
    pub id: String,
    pub project_id: String,
    pub asset_code: String,
    pub title: String,
    pub taxonomy: CreativeTaxonomy,
    /// Direct internal + external production cost in fen.
    pub production_cost_fen: Fen,
    /// Total internal labor hours invested across all production roles.
    pub production_hours: f64,
    pub created_date: NaiveDate,
    pub status: String,
}

/// Links an internal [`CreativeAsset`] to a platform-specific creative entity and/or shared content ID.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreativePlatformLink {
    pub id: String,
    pub asset_id: String,
    pub platform: PlatformId,
    pub ad_account_id: String,
    /// Platform creative / order / unit ID (matches `unified_reports.entity_id`).
    pub platform_creative_id: String,
    /// Cross-platform content handle (`note_id`, `bvid`, `photo_id`, `material_id`) enabling
    /// KOL seeding + paid boost attribution (e.g. Pugongying + Juguang, Huahuo + Sanlian).
    pub platform_content_id: Option<String>,
}

/// Creative production role used for workload & output attribution.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductionRole {
    Scriptwriter,
    Shooter,
    Editor,
    Designer,
    Actor,
    Director,
    AiOperator,
    Producer,
    Custom(String),
}

/// Internal creator workload & credit contribution record for a creative asset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreativeProductionContribution {
    pub id: String,
    pub asset_id: String,
    pub person_id: String,
    pub role: ProductionRole,
    pub work_hours: f64,
    /// Share of creative output credit assigned to this contributor (`0.0..=1.0`, sums to 1.0 per asset).
    pub credit_weight: f64,
    /// Optional piece-rate or studio allocation fee in fen.
    pub piece_cost_fen: Fen,
}

/// Per-creative attribution rollup connecting production effort to media performance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreativeAttributionRow {
    pub asset_id: String,
    pub asset_code: String,
    pub title: String,
    pub project_id: String,
    pub format: CreativeFormat,
    pub hook_type: CreativeHookType,
    pub scene_tag: String,
    pub production_hours: f64,
    pub production_cost_fen: Fen,
    pub media_gross_cost_fen: Fen,
    pub kol_seeding_cost_fen: Fen,
    pub impressions: u64,
    pub clicks: u64,
    pub interactions: u64,
    pub conversions: u64,
    pub gmv_fen: Fen,
    pub ctr: f64,
    pub roi: f64,
    /// GMV generated per hour of creative production labor (`gmv_yuan / production_hours`).
    pub gmv_per_hour_yuan: f64,
    /// Media spend carried per hour of production (`gross_cost_yuan / production_hours`).
    pub spend_carried_per_hour_yuan: f64,
    /// Production cost as % of total media + seeding cost.
    pub production_cost_ratio: f64,
}

/// Per-creator workload & attributed ROI summary across all assets they contributed to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatorWorkloadSummary {
    pub person_id: String,
    pub person_name: String,
    pub department: String,
    pub assets_touched: usize,
    pub total_work_hours: f64,
    pub weighted_piece_cost_fen: Fen,
    pub attributed_media_spend_fen: Fen,
    pub attributed_gmv_fen: Fen,
    pub attributed_conversions: f64,
    pub gmv_per_work_hour_yuan: f64,
    pub spend_per_work_hour_yuan: f64,
    pub avg_attributed_roi: f64,
}

/// Cross-platform seeding + paid boost attribution joined by `content_id` (`note_id` or `bvid`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedingBoostAttributionRow {
    pub content_id: String,
    pub seeding_platform: PlatformId,
    pub boost_platform: Option<PlatformId>,
    pub kol_order_fee_fen: Fen,
    pub kol_service_fee_fen: Fen,
    pub paid_boost_cost_fen: Fen,
    pub total_full_funnel_cost_fen: Fen,
    pub total_impressions: u64,
    pub total_clicks: u64,
    pub total_interactions: u64,
    pub total_conversions: u64,
    pub total_gmv_fen: Fen,
    pub full_funnel_cpe_yuan: f64,
    pub full_funnel_roi: f64,
}
