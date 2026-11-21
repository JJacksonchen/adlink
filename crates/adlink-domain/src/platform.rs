//! Supported Chinese advertising platforms, official API metadata, and canonical field mappings.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
#[error("unknown platform id '{0}'")]
pub struct ParsePlatformError(pub String);

/// Supported advertising & creator-marketing platforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformId {
    /// 腾讯社交广告 (Tencent Ads / AMS Marketing API v3.0)
    TencentAds,
    /// 小红书聚光 (Xiaohongshu Juguang Marketing API)
    XhsJuguang,
    /// 小红书乘风 (Xiaohongshu Chengfeng E-Commerce Marketing API)
    XhsChengfeng,
    /// 小红书蒲公英 (Xiaohongshu Pugongying KOL Collaboration)
    XhsPugongying,
    /// B站广告后台三连 / 商业起飞 (Bilibili Sanlian / Qifei MAPI)
    BilibiliSanlian,
    /// B站花火商单平台 (Bilibili Huahuo Creator Platform)
    BilibiliHuahuo,
    /// 快手磁力引擎 (Kuaishou Magnetic Engine Marketing API v1)
    Kuaishou,
    /// 巨量引擎 AD (Ocean Engine AD Marketing API v3.0)
    OceanEngineAd,
    /// 巨量本地推 (Ocean Engine Local Ads v3.0)
    OceanEngineLocal,
    /// 支付宝灯火 (Alipay Denghuo Ad OpenAPI)
    AlipayDenghuo,
}

impl PlatformId {
    /// All 10 supported platform connectors in canonical display order.
    pub const ALL: &'static [Self] = &[
        Self::TencentAds,
        Self::XhsJuguang,
        Self::XhsChengfeng,
        Self::XhsPugongying,
        Self::BilibiliSanlian,
        Self::BilibiliHuahuo,
        Self::Kuaishou,
        Self::OceanEngineAd,
        Self::OceanEngineLocal,
        Self::AlipayDenghuo,
    ];

    /// Canonical snake_case identifier used across APIs, DB keys, and MCP tools.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::TencentAds => "tencent_ads",
            Self::XhsJuguang => "xhs_juguang",
            Self::XhsChengfeng => "xhs_chengfeng",
            Self::XhsPugongying => "xhs_pugongying",
            Self::BilibiliSanlian => "bilibili_sanlian",
            Self::BilibiliHuahuo => "bilibili_huahuo",
            Self::Kuaishou => "kuaishou",
            Self::OceanEngineAd => "ocean_engine_ad",
            Self::OceanEngineLocal => "ocean_engine_local",
            Self::AlipayDenghuo => "alipay_denghuo",
        }
    }

    /// Official Chinese platform name.
    pub const fn display_name_zh(&self) -> &'static str {
        match self {
            Self::TencentAds => "腾讯社交广告 (AMS v3.0)",
            Self::XhsJuguang => "小红书聚光",
            Self::XhsChengfeng => "小红书乘风",
            Self::XhsPugongying => "小红书蒲公英",
            Self::BilibiliSanlian => "B站三连/商业起飞",
            Self::BilibiliHuahuo => "B站花火平台",
            Self::Kuaishou => "快手磁力引擎",
            Self::OceanEngineAd => "巨量引擎 AD",
            Self::OceanEngineLocal => "巨量本地推",
            Self::AlipayDenghuo => "支付宝灯火",
        }
    }

    /// Official English platform name.
    pub const fn display_name_en(&self) -> &'static str {
        match self {
            Self::TencentAds => "Tencent Ads (Marketing API v3.0)",
            Self::XhsJuguang => "Xiaohongshu Juguang",
            Self::XhsChengfeng => "Xiaohongshu Chengfeng",
            Self::XhsPugongying => "Xiaohongshu Pugongying",
            Self::BilibiliSanlian => "Bilibili Sanlian / Qifei",
            Self::BilibiliHuahuo => "Bilibili Huahuo",
            Self::Kuaishou => "Kuaishou Magnetic Engine",
            Self::OceanEngineAd => "Ocean Engine AD (v3.0)",
            Self::OceanEngineLocal => "Ocean Engine Local Ads (v3.0)",
            Self::AlipayDenghuo => "Alipay Denghuo",
        }
    }

    /// Returns official platform specification, constraints, and field mapping metadata.
    pub fn meta(&self) -> PlatformMeta {
        platform_meta(*self)
    }
}

impl fmt::Display for PlatformId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PlatformId {
    type Err = ParsePlatformError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.trim().to_ascii_lowercase().replace('-', "_");
        match normalized.as_str() {
            "tencent_ads" | "tencent" | "ams" | "gdt" => Ok(Self::TencentAds),
            "xhs_juguang" | "juguang" | "xiaohongshu_juguang" => Ok(Self::XhsJuguang),
            "xhs_chengfeng" | "chengfeng" | "xiaohongshu_chengfeng" => Ok(Self::XhsChengfeng),
            "xhs_pugongying" | "pugongying" | "pgy" => Ok(Self::XhsPugongying),
            "bilibili_sanlian" | "sanlian" | "bilibili_qifei" | "qifei" => Ok(Self::BilibiliSanlian),
            "bilibili_huahuo" | "huahuo" => Ok(Self::BilibiliHuahuo),
            "kuaishou" | "ks" | "magnetic" => Ok(Self::Kuaishou),
            "ocean_engine_ad" | "ocean_ad" | "oceanengine" | "juliang" | "ad" => {
                Ok(Self::OceanEngineAd)
            }
            "ocean_engine_local" | "ocean_local" | "local_ads" | "benditui" => {
                Ok(Self::OceanEngineLocal)
            }
            "alipay_denghuo" | "denghuo" | "alipay" => Ok(Self::AlipayDenghuo),
            _ => Err(ParsePlatformError(s.to_string())),
        }
    }
}

/// Honesty-first indicator describing official API openness for each platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiAccessMode {
    /// Fully documented self-serve public Marketing API.
    OfficialPublicApi,
    /// Official Marketing API requiring enterprise/agency whitelist approval.
    OfficialPartnerApi,
    /// No public self-serve MAPI; supports partner whitelisted endpoint + structured CSV/JSON import + mock.
    HybridImportAndPartnerApi,
}

/// Official field mapping entry documenting exact upstream naming and units.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfficialFieldMapping {
    pub canonical_field: &'static str,
    pub official_field: &'static str,
    pub official_unit: &'static str,
    pub notes: &'static str,
}

/// Complete platform descriptor used by the sync chunker, rate limiter, API, and Agent onboarding guides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformMeta {
    pub id: PlatformId,
    pub name_zh: &'static str,
    pub name_en: &'static str,
    pub access_mode: ApiAccessMode,
    pub base_url: &'static str,
    pub docs_url: &'static str,
    pub auth_scheme: &'static str,
    pub monetary_unit: &'static str,
    /// Maximum inclusive days permitted per synchronous report request by the official API.
    pub max_sync_days_per_chunk: u32,
    /// Conservative default sustained requests per second per app/token.
    pub default_qps: f64,
    pub supports_ad_mutation: bool,
    pub supports_hourly_report: bool,
    pub official_endpoints: Vec<&'static str>,
    pub field_mappings: Vec<OfficialFieldMapping>,
    pub compliance_note: &'static str,
}

fn platform_meta(id: PlatformId) -> PlatformMeta {
    match id {
        PlatformId::TencentAds => PlatformMeta {
            id,
            name_zh: id.display_name_zh(),
            name_en: id.display_name_en(),
            access_mode: ApiAccessMode::OfficialPublicApi,
            base_url: "https://api.e.qq.com/v3.0",
            docs_url: "https://developers.e.qq.com/docs/api/insights/ad_insights/daily_reports_get",
            auth_scheme: "OAuth 2.0 (access_token + timestamp + nonce)",
            monetary_unit: "fen (分)",
            max_sync_days_per_chunk: 90,
            default_qps: 5.0,
            supports_ad_mutation: true,
            supports_hourly_report: true,
            official_endpoints: vec![
                "GET /v3.0/daily_reports/get",
                "GET /v3.0/hourly_reports/get",
                "GET /v3.0/funds/get",
                "GET /v3.0/daily_balance_report/get",
                "GET /v3.0/adgroups/get",
                "POST /v3.0/adgroups/update",
                "POST /v3.0/async_tasks/add",
            ],
            field_mappings: vec![
                OfficialFieldMapping {
                    canonical_field: "gross_cost_fen",
                    official_field: "cost",
                    official_unit: "fen",
                    notes: "v3.0 new metric system (already in fen)",
                },
                OfficialFieldMapping {
                    canonical_field: "impressions",
                    official_field: "view_count",
                    official_unit: "count",
                    notes: "Replaces legacy v1/v2 impression",
                },
                OfficialFieldMapping {
                    canonical_field: "clicks",
                    official_field: "valid_click_count",
                    official_unit: "count",
                    notes: "Replaces legacy click",
                },
                OfficialFieldMapping {
                    canonical_field: "conversions",
                    official_field: "conversions_count",
                    official_unit: "count",
                    notes: "Supports REPORTING_TIME and REQUEST_TIME time_line",
                },
                OfficialFieldMapping {
                    canonical_field: "cash_cost_fen / grant_cost_fen",
                    official_field: "funds/get -> GENERAL_CASH / GENERAL_GIFT",
                    official_unit: "fen",
                    notes: "Reconciled via daily_balance_report/get",
                },
            ],
            compliance_note: "Official AMS v3.0 recommends syncing full D-1 daily reports after 08:00 CST.",
        },
        PlatformId::XhsJuguang => PlatformMeta {
            id,
            name_zh: id.display_name_zh(),
            name_en: id.display_name_en(),
            access_mode: ApiAccessMode::OfficialPartnerApi,
            base_url: "https://adapi.xiaohongshu.com",
            docs_url: "https://open.xiaohongshu.com/",
            auth_scheme: "OAuth 2.0 (Header: Access-Token)",
            monetary_unit: "fen (分) on balance / fen or yuan per whitepaper",
            max_sync_days_per_chunk: 30,
            default_qps: 4.0,
            supports_ad_mutation: true,
            supports_hourly_report: true,
            official_endpoints: vec![
                "POST /api/open/jg/account/balance/info",
                "POST /api/open/jg/data/report/offline/account",
                "POST /api/open/jg/data/report/offline/campaign",
                "POST /api/open/jg/data/report/offline/unit",
                "POST /api/open/jg/data/report/offline/creative",
                "POST /api/open/jg/unit/update/status",
            ],
            field_mappings: vec![
                OfficialFieldMapping {
                    canonical_field: "gross_cost_fen",
                    official_field: "fee",
                    official_unit: "fen",
                    notes: "Account balance & spend normalized to integer fen",
                },
                OfficialFieldMapping {
                    canonical_field: "impressions",
                    official_field: "impression",
                    official_unit: "count",
                    notes: "Ad impression count",
                },
                OfficialFieldMapping {
                    canonical_field: "clicks",
                    official_field: "click",
                    official_unit: "count",
                    notes: "Ad click count",
                },
                OfficialFieldMapping {
                    canonical_field: "interactions",
                    official_field: "interaction",
                    official_unit: "count",
                    notes: "Likes + collects + comments + follows on promoted note",
                },
                OfficialFieldMapping {
                    canonical_field: "content_id",
                    official_field: "note_id",
                    official_unit: "string",
                    notes: "Joins paid Juguang traffic with Pugongying KOL seeding note",
                },
            ],
            compliance_note: "Requires advertiser/agency whitelist approval on XHS Open Platform.",
        },
        PlatformId::XhsChengfeng => PlatformMeta {
            id,
            name_zh: id.display_name_zh(),
            name_en: id.display_name_en(),
            access_mode: ApiAccessMode::OfficialPartnerApi,
            base_url: "https://adapi.xiaohongshu.com",
            docs_url: "https://open.xiaohongshu.com/",
            auth_scheme: "OAuth 2.0 (Header: Access-Token)",
            monetary_unit: "fen (分)",
            max_sync_days_per_chunk: 30,
            default_qps: 4.0,
            supports_ad_mutation: true,
            supports_hourly_report: true,
            official_endpoints: vec![
                "POST /api/open/jg/account/balance/info",
                "POST /api/open/cf/data/report/offline/campaign",
                "POST /api/open/cf/data/report/offline/creative",
                "POST /api/open/jg/unit/update/status",
            ],
            field_mappings: vec![
                OfficialFieldMapping {
                    canonical_field: "gross_cost_fen",
                    official_field: "fee",
                    official_unit: "fen",
                    notes: "Shared XHS fund pool with Juguang",
                },
                OfficialFieldMapping {
                    canonical_field: "orders",
                    official_field: "deal_order_num",
                    official_unit: "count",
                    notes: "Chengfeng closed-loop e-commerce orders",
                },
                OfficialFieldMapping {
                    canonical_field: "gmv_fen",
                    official_field: "deal_order_gmv",
                    official_unit: "fen",
                    notes: "Chengfeng 7d/15d attributed GMV",
                },
                OfficialFieldMapping {
                    canonical_field: "content_id",
                    official_field: "note_id",
                    official_unit: "string",
                    notes: "Links SPU/note seeding creative to transaction GMV",
                },
            ],
            compliance_note: "Shares XHS underlying fund wallet; e-commerce GMV metrics require Chengfeng scopes.",
        },
        PlatformId::XhsPugongying => PlatformMeta {
            id,
            name_zh: id.display_name_zh(),
            name_en: id.display_name_en(),
            access_mode: ApiAccessMode::HybridImportAndPartnerApi,
            base_url: "https://pgy.xiaohongshu.com (Partner API / Structured Import)",
            docs_url: "https://open.xiaohongshu.com/",
            auth_scheme: "Partner OAuth 2.0 Token OR Structured CSV/JSON Ingest",
            monetary_unit: "fen (分)",
            max_sync_days_per_chunk: 30,
            default_qps: 2.0,
            supports_ad_mutation: false,
            supports_hourly_report: false,
            official_endpoints: vec![
                "Partner Whitelist Endpoint (configurable)",
                "POST /api/v1/ingest (Structured PGY export / partner webhook)",
            ],
            field_mappings: vec![
                OfficialFieldMapping {
                    canonical_field: "entity_id",
                    official_field: "order_id",
                    official_unit: "string",
                    notes: "Pugongying KOL collaboration order ID",
                },
                OfficialFieldMapping {
                    canonical_field: "content_id",
                    official_field: "note_id",
                    official_unit: "string",
                    notes: "Shared note_id enabling seeding + Juguang/Chengfeng boost attribution",
                },
                OfficialFieldMapping {
                    canonical_field: "kol_order_fee_fen",
                    official_field: "kol_quote_fee",
                    official_unit: "fen",
                    notes: "Creator collaboration fee",
                },
                OfficialFieldMapping {
                    canonical_field: "platform_service_fee_fen",
                    official_field: "platform_service_fee",
                    official_unit: "fen",
                    notes: "Pugongying platform 10% service charge",
                },
                OfficialFieldMapping {
                    canonical_field: "interactions",
                    official_field: "like_cnt + collect_cnt + comment_cnt",
                    official_unit: "count",
                    notes: "Organic + boosted note engagement",
                },
            ],
            compliance_note: "XHS Pugongying does not offer an open self-serve public API; AdLink never scrapes and strictly supports whitelisted partner APIs or official CSV/JSON ingestion.",
        },
        PlatformId::BilibiliSanlian => PlatformMeta {
            id,
            name_zh: id.display_name_zh(),
            name_en: id.display_name_en(),
            access_mode: ApiAccessMode::OfficialPartnerApi,
            base_url: "https://cm.bilibili.com/takumi/api",
            docs_url: "https://openhome.bilibili.com/",
            auth_scheme: "Bearer Token + X-Call-Source Header (Official Commercial Partner)",
            monetary_unit: "fen (分)",
            max_sync_days_per_chunk: 30,
            default_qps: 4.0,
            supports_ad_mutation: true,
            supports_hourly_report: true,
            official_endpoints: vec![
                "POST /takumi/api/report/query",
                "GET /takumi/api/account/balance",
                "POST /takumi/api/campaign/status",
            ],
            field_mappings: vec![
                OfficialFieldMapping {
                    canonical_field: "gross_cost_fen",
                    official_field: "cost",
                    official_unit: "fen",
                    notes: "Ad spend in fen",
                },
                OfficialFieldMapping {
                    canonical_field: "impressions",
                    official_field: "impression",
                    official_unit: "count",
                    notes: "Bilibili feed/player exposure count",
                },
                OfficialFieldMapping {
                    canonical_field: "clicks",
                    official_field: "click",
                    official_unit: "count",
                    notes: "Ad card / blue-link clicks",
                },
                OfficialFieldMapping {
                    canonical_field: "interactions",
                    official_field: "like_cnt + coin_cnt + fav_cnt + share_cnt",
                    official_unit: "count",
                    notes: "Bilibili Sanlian engagement metrics",
                },
                OfficialFieldMapping {
                    canonical_field: "content_id",
                    official_field: "bvid",
                    official_unit: "string",
                    notes: "Joins Qifei/Sanlian paid boost with Huahuo商单",
                },
            ],
            compliance_note: "Requires Bilibili commercial partner/agency whitelist authorization on cm.bilibili.com / openhome.bilibili.com.",
        },
        PlatformId::BilibiliHuahuo => PlatformMeta {
            id,
            name_zh: id.display_name_zh(),
            name_en: id.display_name_en(),
            access_mode: ApiAccessMode::HybridImportAndPartnerApi,
            base_url: "https://huahuo.bilibili.com (Partner API / Structured Import)",
            docs_url: "https://e.bilibili.com/",
            auth_scheme: "Commercial Partner Token OR Structured Export Ingest",
            monetary_unit: "fen (分)",
            max_sync_days_per_chunk: 30,
            default_qps: 2.0,
            supports_ad_mutation: false,
            supports_hourly_report: false,
            official_endpoints: vec![
                "Partner Whitelist Endpoint (configurable)",
                "POST /api/v1/ingest (Structured Huahuo商单 settlement export)",
            ],
            field_mappings: vec![
                OfficialFieldMapping {
                    canonical_field: "entity_id",
                    official_field: "huahuo_order_id",
                    official_unit: "string",
                    notes: "Huahuo UP-creator commercial order ID",
                },
                OfficialFieldMapping {
                    canonical_field: "content_id",
                    official_field: "bvid",
                    official_unit: "string",
                    notes: "Shared BV video ID for Huahuo + Qifei full-funnel attribution",
                },
                OfficialFieldMapping {
                    canonical_field: "kol_order_fee_fen",
                    official_field: "order_amount",
                    official_unit: "fen",
                    notes: "UP creator fee",
                },
                OfficialFieldMapping {
                    canonical_field: "platform_service_fee_fen",
                    official_field: "platform_fee",
                    official_unit: "fen",
                    notes: "Huahuo service fee",
                },
                OfficialFieldMapping {
                    canonical_field: "clicks",
                    official_field: "blue_link_click_cnt",
                    official_unit: "count",
                    notes: "Pinned comment / player blue-link clicks",
                },
            ],
            compliance_note: "Bilibili Huahuo lacks an open public self-serve API; AdLink provides compliant partner endpoint hook + structured export ingestion.",
        },
        PlatformId::Kuaishou => PlatformMeta {
            id,
            name_zh: id.display_name_zh(),
            name_en: id.display_name_en(),
            access_mode: ApiAccessMode::OfficialPublicApi,
            base_url: "https://ad.e.kuaishou.com/rest/openapi",
            docs_url: "https://developers.e.kuaishou.com/",
            auth_scheme: "OAuth 2.0 (Header: Access-Token)",
            monetary_unit: "yuan (元, float) -> converted to fen (分)",
            max_sync_days_per_chunk: 7,
            default_qps: 5.0,
            supports_ad_mutation: true,
            supports_hourly_report: true,
            official_endpoints: vec![
                "POST /rest/openapi/v1/report/account_report",
                "POST /rest/openapi/v1/report/campaign_report",
                "POST /rest/openapi/v1/report/unit_report",
                "POST /rest/openapi/v1/report/creative_report",
                "POST /rest/openapi/v1/campaign/update/status",
            ],
            field_mappings: vec![
                OfficialFieldMapping {
                    canonical_field: "gross_cost_fen",
                    official_field: "charge",
                    official_unit: "yuan (converted *100 to fen)",
                    notes: "Kuaishou real-time & offline charge",
                },
                OfficialFieldMapping {
                    canonical_field: "impressions",
                    official_field: "show",
                    official_unit: "count",
                    notes: "Ad exposure count",
                },
                OfficialFieldMapping {
                    canonical_field: "clicks",
                    official_field: "photo_click",
                    official_unit: "count",
                    notes: "Video/photo clicks",
                },
                OfficialFieldMapping {
                    canonical_field: "gmv_fen",
                    official_field: "event_pay_purchase_amount",
                    official_unit: "yuan -> fen",
                    notes: "Paid conversion amount",
                },
            ],
            compliance_note: "Official real-time report endpoints restrict span to 7 days; AdLink auto-chunks long date ranges.",
        },
        PlatformId::OceanEngineAd => PlatformMeta {
            id,
            name_zh: id.display_name_zh(),
            name_en: id.display_name_en(),
            access_mode: ApiAccessMode::OfficialPublicApi,
            base_url: "https://api.oceanengine.com",
            docs_url: "https://open.oceanengine.com/labels/7/docs/1741023482480652",
            auth_scheme: "OAuth 2.0 (Header: Access-Token)",
            monetary_unit: "yuan (元, float) -> converted to fen (分)",
            max_sync_days_per_chunk: 30,
            default_qps: 8.0,
            supports_ad_mutation: true,
            supports_hourly_report: true,
            official_endpoints: vec![
                "GET /open_api/v3.0/report/custom/get/",
                "GET /open_api/v3.0/report/custom/get_acl/",
                "GET /open_api/v3.0/project/list/",
                "GET /open_api/v3.0/promotion/list/",
                "GET /open_api/2/advertiser/fund/daily_stat/",
                "POST /open_api/v3.0/promotion/status/update/",
            ],
            field_mappings: vec![
                OfficialFieldMapping {
                    canonical_field: "gross_cost_fen",
                    official_field: "stat_cost",
                    official_unit: "yuan (converted *100 to fen)",
                    notes: "Ocean Engine v3.0 custom report stat_cost",
                },
                OfficialFieldMapping {
                    canonical_field: "impressions",
                    official_field: "show_cnt",
                    official_unit: "count",
                    notes: "Impressions",
                },
                OfficialFieldMapping {
                    canonical_field: "clicks",
                    official_field: "click_cnt",
                    official_unit: "count",
                    notes: "Clicks",
                },
                OfficialFieldMapping {
                    canonical_field: "conversions",
                    official_field: "convert_cnt",
                    official_unit: "count",
                    notes: "Conversions",
                },
                OfficialFieldMapping {
                    canonical_field: "cash_cost_fen / grant_cost_fen",
                    official_field: "daily_stat -> cash_income / grant_income / shared_wallet_cost",
                    official_unit: "yuan -> fen",
                    notes: "Daily fund reconciliation",
                },
            ],
            compliance_note: "Uses Ocean Engine Marketing API v3.0 upgraded Project/Promotion hierarchy.",
        },
        PlatformId::OceanEngineLocal => PlatformMeta {
            id,
            name_zh: id.display_name_zh(),
            name_en: id.display_name_en(),
            access_mode: ApiAccessMode::OfficialPublicApi,
            base_url: "https://api.oceanengine.com",
            docs_url: "https://open.oceanengine.com/",
            auth_scheme: "OAuth 2.0 (Header: Access-Token)",
            monetary_unit: "yuan (元, float) -> converted to fen (分)",
            max_sync_days_per_chunk: 30,
            default_qps: 6.0,
            supports_ad_mutation: true,
            supports_hourly_report: true,
            official_endpoints: vec![
                "GET /open_api/v3.0/local/report/project/get/",
                "GET /open_api/v3.0/local/report/promotion/get/",
                "GET /open_api/v3.0/local/project/list/",
                "POST /open_api/v3.0/local/promotion/status/update/",
            ],
            field_mappings: vec![
                OfficialFieldMapping {
                    canonical_field: "gross_cost_fen",
                    official_field: "stat_cost",
                    official_unit: "yuan -> fen",
                    notes: "Local Ads project/promotion spend",
                },
                OfficialFieldMapping {
                    canonical_field: "gmv_fen",
                    official_field: "pay_order_amount",
                    official_unit: "yuan -> fen",
                    notes: "Group-buy / local life order GMV",
                },
                OfficialFieldMapping {
                    canonical_field: "store_verifies",
                    official_field: "local_life_verify_cnt",
                    official_unit: "count",
                    notes: "Offline store verification count",
                },
            ],
            compliance_note: "Requires Ocean Engine Local Ads (巨量本地推) OAuth scopes.",
        },
        PlatformId::AlipayDenghuo => PlatformMeta {
            id,
            name_zh: id.display_name_zh(),
            name_en: id.display_name_en(),
            access_mode: ApiAccessMode::OfficialPublicApi,
            base_url: "https://openapi.alipay.com/gateway.do",
            docs_url: "https://open.alipay.com/",
            auth_scheme: "Alipay OpenAPI RSA2 (SHA256WithRSA) + biz_token + alipay_pid",
            monetary_unit: "fen (分)",
            max_sync_days_per_chunk: 7,
            default_qps: 5.0,
            supports_ad_mutation: true,
            supports_hourly_report: true,
            official_endpoints: vec![
                "alipay.data.dataservice.ad.reportdata.query",
                "alipay.data.dataservice.ad.reportmetric.query",
                "alipay.data.dataservice.ad.consumehistory.query",
                "alipay.data.dataservice.ad.principalconsume.query",
            ],
            field_mappings: vec![
                OfficialFieldMapping {
                    canonical_field: "gross_cost_fen",
                    official_field: "data_list[].cost",
                    official_unit: "fen",
                    notes: "Denghuo performance spend in fen",
                },
                OfficialFieldMapping {
                    canonical_field: "impressions",
                    official_field: "data_list[].impression",
                    official_unit: "count",
                    notes: "Ad impression count",
                },
                OfficialFieldMapping {
                    canonical_field: "clicks",
                    official_field: "data_list[].click",
                    official_unit: "count",
                    notes: "Ad click count",
                },
                OfficialFieldMapping {
                    canonical_field: "conversions",
                    official_field: "data_list[].conv_result / conv_data_list",
                    official_unit: "count",
                    notes: "Supports conv_code_list and conv_time_join_rule",
                },
            ],
            compliance_note: "Enforces max 7-day range for DETAIL and 1-day for HOUR; dates formatted yyyyMMdd; RSA2 signed.",
        },
    }
}
