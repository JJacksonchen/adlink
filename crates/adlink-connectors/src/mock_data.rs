//! Deterministic realistic agency simulation dataset (`星图智投 / StarPulse Digital`)
//! covering all 10 connectors, `人-户关系`, `项目类型分类`, `创意类型分类`, internal production
//! workload credits, and cross-platform seeding+boost attribution (`note_id`/`bvid`).

use std::collections::BTreeMap;

use adlink_domain::{
    AccountType, AdAccount, AdEntity, BindingRole, CaliberCategory, CreativeAsset, CreativeFormat,
    CreativeHookType, CreativePlatformLink, CreativeProductionContribution, CreativeTaxonomy,
    CustomCaliberDef, Fen, OrgType, Organization, Person, PersonAccountBinding, PersonRole,
    PlatformId, ProductionRole, Project, ProjectAccountLink, ProjectType, ReportLevel,
    SettlementMode,
};
use adlink_storage::{StorageError, StorageService};
use chrono::{Datelike, NaiveDate, Utc};
use serde_json::json;

/// Simple deterministic PRNG (SplitMix64) so mock data is 100% reproducible across runs & tests.
#[derive(Clone)]
pub struct DeterministicRng(u64);

impl DeterministicRng {
    pub fn from_seed(seed: u64) -> Self {
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn next_f64(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64) / ((1u64 << 53) as f64)
    }

    pub fn range_f64(&mut self, min: f64, max: f64) -> f64 {
        min + (max - min) * self.next_f64()
    }
}

/// Deterministic hash seed from platform, account, date, and entity ID.
pub fn stable_seed(tag: &str, date: NaiveDate, idx: usize) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in tag.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h ^= date.num_days_from_ce() as u64;
    h = h.wrapping_mul(0x100000001b3);
    h ^ (idx as u64)
}

/// Generates deterministic seasonal multiplier (double-11 / weekend surges) for realistic trends.
pub fn seasonal_factor(date: NaiveDate) -> f64 {
    let weekday_boost = match date.weekday() {
        chrono::Weekday::Fri | chrono::Weekday::Sat => 1.18,
        chrono::Weekday::Sun => 1.10,
        _ => 0.96,
    };
    let promo_boost = if (date.month() == 11 && date.day() <= 12)
        || (date.month() == 6 && date.day() <= 18)
    {
        1.45
    } else {
        1.0
    };
    weekday_boost * promo_boost
}

pub fn mock_account_id(platform: PlatformId) -> &'static str {
    match platform {
        PlatformId::TencentAds => "acc-tencent-01",
        PlatformId::XhsJuguang => "acc-xhs-jg-01",
        PlatformId::XhsChengfeng => "acc-xhs-cf-01",
        PlatformId::XhsPugongying => "acc-xhs-pgy-01",
        PlatformId::BilibiliSanlian => "acc-bili-sl-01",
        PlatformId::BilibiliHuahuo => "acc-bili-hh-01",
        PlatformId::Kuaishou => "acc-kuaishou-01",
        PlatformId::OceanEngineAd => "acc-ocean-ad-01",
        PlatformId::OceanEngineLocal => "acc-ocean-local-01",
        PlatformId::AlipayDenghuo => "acc-alipay-dh-01",
        PlatformId::TikTokAds => "acc-tiktok-01",
        PlatformId::MetaAds => "acc-meta-01",
        PlatformId::GoogleAds => "acc-google-01",
        PlatformId::AppleSearchAds => "acc-asa-01",
        PlatformId::AmazonAds => "acc-amazon-01",
    }
}

pub fn mock_platform_account_id(platform: PlatformId) -> &'static str {
    match platform {
        PlatformId::TencentAds => "38291044",
        PlatformId::XhsJuguang => "670192831",
        PlatformId::XhsChengfeng => "670192832",
        PlatformId::XhsPugongying => "880192001",
        PlatformId::BilibiliSanlian => "50129384",
        PlatformId::BilibiliHuahuo => "50129385",
        PlatformId::Kuaishou => "21904851",
        PlatformId::OceanEngineAd => "178902341001",
        PlatformId::OceanEngineLocal => "178902341002",
        PlatformId::AlipayDenghuo => "2088612093847102",
        PlatformId::TikTokAds => "7391028374619201",
        PlatformId::MetaAds => "act_918273645019283",
        PlatformId::GoogleAds => "7428910345",
        PlatformId::AppleSearchAds => "4910283",
        PlatformId::AmazonAds => "3910294857123456",
    }
}

/// Returns deterministic mock campaigns/creatives/content IDs for each account.
pub fn mock_entities_for_account(account: &AdAccount) -> Vec<AdEntity> {
    let p = account.platform;
    let now = Utc::now();
    match p {
        PlatformId::TencentAds => vec![
            entity(p, &account.id, ReportLevel::Campaign, "tc_camp_01", "微信朋友圈_秋冬精华首单", None, 250_000, 45_00, now, json!({"bid_mode":"OCPM"})),
            entity(p, &account.id, ReportLevel::AdGroup, "tc_adg_01", "25-40岁一二线护肤人群", Some("tc_camp_01"), 120_000, 45_00, now, json!({})),
            entity(p, &account.id, ReportLevel::Creative, "tc_cre_01", "实验室成分拆解视频A", Some("tc_adg_01"), 60_000, 45_00, now, json!({"content_id":"mat_tc_01"})),
            entity(p, &account.id, ReportLevel::Creative, "tc_cre_02", "熬夜肌对比测评竖版", Some("tc_adg_01"), 60_000, 45_00, now, json!({"content_id":"mat_tc_02"})),
        ],
        PlatformId::XhsJuguang => vec![
            entity(p, &account.id, ReportLevel::Campaign, "jg_camp_01", "聚光_抗老精华种草搜索", None, 180_000, 32_00, now, json!({})),
            entity(p, &account.id, ReportLevel::AdGroup, "jg_unit_01", "搜索词_早C晚A/熬夜精华", Some("jg_camp_01"), 90_000, 32_00, now, json!({})),
            entity(p, &account.id, ReportLevel::Creative, "jg_cre_01", "聚光加热_PGY爆文Note101", Some("jg_unit_01"), 45_000, 32_00, now, json!({"note_id":"note_xhs_101"})),
            entity(p, &account.id, ReportLevel::Creative, "jg_cre_02", "聚光图文_通勤妆容测评", Some("jg_unit_01"), 45_000, 32_00, now, json!({"note_id":"note_xhs_102"})),
        ],
        PlatformId::XhsChengfeng => vec![
            entity(p, &account.id, ReportLevel::Campaign, "cf_camp_01", "乘风_站内闭环成交计划", None, 200_000, 38_00, now, json!({})),
            entity(p, &account.id, ReportLevel::Creative, "cf_cre_01", "乘风直投_PGY爆文Note101收割", Some("cf_camp_01"), 100_000, 38_00, now, json!({"note_id":"note_xhs_101"})),
            entity(p, &account.id, ReportLevel::Creative, "cf_cre_02", "乘风商品卡_礼盒买赠", Some("cf_camp_01"), 100_000, 38_00, now, json!({"note_id":"note_xhs_103"})),
        ],
        PlatformId::XhsPugongying => vec![
            entity(p, &account.id, ReportLevel::ContentOrder, "pgy_ord_101", "蒲公英商单_腰部护肤KOC_林浅", None, 0, 0, now, json!({"note_id":"note_xhs_101","kol_id":"kol_linqian"})),
            entity(p, &account.id, ReportLevel::ContentOrder, "pgy_ord_102", "蒲公英商单_成分党博主_陈默", None, 0, 0, now, json!({"note_id":"note_xhs_102","kol_id":"kol_chenmo"})),
        ],
        PlatformId::BilibiliSanlian => vec![
            entity(p, &account.id, ReportLevel::Campaign, "bl_camp_01", "三连起飞_科技数码新品测评", None, 150_000, 52_00, now, json!({})),
            entity(p, &account.id, ReportLevel::Creative, "bl_cre_01", "起飞加热_花火BV1xx411c7mD", Some("bl_camp_01"), 80_000, 52_00, now, json!({"bvid":"BV1xx411c7mD"})),
            entity(p, &account.id, ReportLevel::Creative, "bl_cre_02", "三连信息流_桌面改造清单", Some("bl_camp_01"), 70_000, 52_00, now, json!({"bvid":"BV1yy411c9kQ"})),
        ],
        PlatformId::BilibiliHuahuo => vec![
            entity(p, &account.id, ReportLevel::ContentOrder, "hh_ord_01", "花火商单_百大科技UP主深度评测", None, 0, 0, now, json!({"bvid":"BV1xx411c7mD","up_mid":"2948102"})),
            entity(p, &account.id, ReportLevel::ContentOrder, "hh_ord_02", "花火商单_居家桌面好物分享", None, 0, 0, now, json!({"bvid":"BV1yy411c9kQ","up_mid":"3819204"})),
        ],
        PlatformId::Kuaishou => vec![
            entity(p, &account.id, ReportLevel::Campaign, "ks_camp_01", "磁力引擎_快手小店爆品直播引流", None, 220_000, 35_00, now, json!({})),
            entity(p, &account.id, ReportLevel::AdGroup, "ks_unit_01", "新线城市家庭消费人群", Some("ks_camp_01"), 110_000, 35_00, now, json!({})),
            entity(p, &account.id, ReportLevel::Creative, "ks_cre_01", "快手老铁工厂溯源短视频", Some("ks_unit_01"), 60_000, 35_00, now, json!({"photo_id":"ks_photo_801"})),
            entity(p, &account.id, ReportLevel::Creative, "ks_cre_02", "直播切片_限时破价福利", Some("ks_unit_01"), 50_000, 35_00, now, json!({"photo_id":"ks_photo_802"})),
        ],
        PlatformId::OceanEngineAd => vec![
            entity(p, &account.id, ReportLevel::Campaign, "oe_proj_01", "巨量AD_全店优品放量项目", None, 400_000, 48_00, now, json!({})),
            entity(p, &account.id, ReportLevel::AdGroup, "oe_prom_01", "自动投放_高净值女性美妆", Some("oe_proj_01"), 200_000, 48_00, now, json!({})),
            entity(p, &account.id, ReportLevel::Creative, "oe_cre_01", "3秒痛点开场+实验室实测", Some("oe_prom_01"), 120_000, 48_00, now, json!({"material_id":"mat_oe_901"})),
            entity(p, &account.id, ReportLevel::Creative, "oe_cre_02", "创始人故事+周年礼盒", Some("oe_prom_01"), 80_000, 48_00, now, json!({"material_id":"mat_oe_902"})),
        ],
        PlatformId::OceanEngineLocal => vec![
            entity(p, &account.id, ReportLevel::Campaign, "loc_proj_01", "巨量本地推_华东20城门店团购", None, 160_000, 28_00, now, json!({})),
            entity(p, &account.id, ReportLevel::AdGroup, "loc_prom_01", "上海南京西路3km门店引流", Some("loc_proj_01"), 80_000, 28_00, now, json!({})),
            entity(p, &account.id, ReportLevel::Creative, "loc_cre_01", "探店达人套餐核销Vlog", Some("loc_prom_01"), 80_000, 28_00, now, json!({"poi_id":"poi_sh_001"})),
        ],
        PlatformId::AlipayDenghuo => vec![
            entity(p, &account.id, ReportLevel::Campaign, "dh_plan_01", "灯火_支付宝消费券小程序拉新", None, 140_000, 30_00, now, json!({})),
            entity(p, &account.id, ReportLevel::AdGroup, "dh_grp_01", "支付成功页+出行频道定向", Some("dh_plan_01"), 70_000, 30_00, now, json!({})),
            entity(p, &account.id, ReportLevel::Creative, "dh_cre_01", "支付立减5元权益卡片", Some("dh_grp_01"), 70_000, 30_00, now, json!({"material_id":"mat_dh_01"})),
        ],
        PlatformId::TikTokAds => vec![
            entity(p, &account.id, ReportLevel::Campaign, "tt_camp_01", "TikTok_US_DTC_Serum_ABO", None, 362_500, 87_00, now, json!({"objective_type":"WEB_CONVERSIONS"})),
            entity(p, &account.id, ReportLevel::AdGroup, "tt_adg_01", "US_Women_25_44_Skincare", Some("tt_camp_01"), 181_250, 87_00, now, json!({})),
            entity(p, &account.id, ReportLevel::Creative, "tt_ad_01", "UGC_BeforeAfter_GlowSerum_EN", Some("tt_adg_01"), 181_250, 87_00, now, json!({"video_id":"v_tt_001"})),
        ],
        PlatformId::MetaAds => vec![
            entity(p, &account.id, ReportLevel::Campaign, "fb_camp_01", "Meta_US_CBO_AdvantagePlus_DTC", None, 580_000, 108_75, now, json!({"objective":"OUTCOME_SALES"})),
            entity(p, &account.id, ReportLevel::AdGroup, "fb_adset_01", "Broad_US_IG_Reels_Purchasers", Some("fb_camp_01"), 290_000, 108_75, now, json!({})),
            entity(p, &account.id, ReportLevel::Creative, "fb_ad_01", "Reels_FounderStory_SerumBottle", Some("fb_adset_01"), 290_000, 108_75, now, json!({"creative_id":"mat_fb_01"})),
        ],
        PlatformId::GoogleAds => vec![
            entity(p, &account.id, ReportLevel::Campaign, "gg_camp_01", "Google_PMax_US_Skincare_Store", None, 435_000, 72_50, now, json!({"advertising_channel_type":"PERFORMANCE_MAX"})),
            entity(p, &account.id, ReportLevel::AdGroup, "gg_adg_01", "Search_Exact_Peptide_Serum", Some("gg_camp_01"), 217_500, 72_50, now, json!({})),
            entity(p, &account.id, ReportLevel::Creative, "gg_ad_01", "YouTube_Shorts_ClinicTrial_30s", Some("gg_adg_01"), 217_500, 72_50, now, json!({"asset_id":"mat_gg_01"})),
        ],
        PlatformId::AppleSearchAds => vec![
            entity(p, &account.id, ReportLevel::Campaign, "asa_camp_01", "ASA_US_Exact_BeautyApp_Brand", None, 217_500, 29_00, now, json!({})),
            entity(p, &account.id, ReportLevel::AdGroup, "asa_adg_01", "US_iOS_CoreKeywords_Exact", Some("asa_camp_01"), 108_750, 29_00, now, json!({})),
            entity(p, &account.id, ReportLevel::Creative, "asa_cre_01", "ASA_CustomProductPage_Trial", Some("asa_adg_01"), 108_750, 29_00, now, json!({"adam_id":"6471092831"})),
        ],
        PlatformId::AmazonAds => vec![
            entity(p, &account.id, ReportLevel::Campaign, "amz_camp_01", "SP_US_Auto_HeroSerum_ASIN_B09X", None, 290_000, 50_75, now, json!({})),
            entity(p, &account.id, ReportLevel::AdGroup, "amz_adg_01", "SP_Exact_VitaminC_Serum", Some("amz_camp_01"), 145_000, 50_75, now, json!({})),
            entity(p, &account.id, ReportLevel::Creative, "amz_ad_01", "ASIN_B09XYZ1234_MainSKU", Some("amz_adg_01"), 145_000, 50_75, now, json!({"asin":"B09XYZ1234"})),
        ],
    }
}

#[allow(clippy::too_many_arguments)]
fn entity(
    platform: PlatformId,
    ad_account_id: &str,
    level: ReportLevel,
    entity_id: &str,
    entity_name: &str,
    parent_entity_id: Option<&str>,
    daily_budget_fen: i64,
    bid_fen: i64,
    updated_at: chrono::DateTime<Utc>,
    raw_extra: serde_json::Value,
) -> AdEntity {
    AdEntity {
        platform,
        ad_account_id: ad_account_id.to_string(),
        level,
        entity_id: entity_id.to_string(),
        entity_name: entity_name.to_string(),
        parent_entity_id: parent_entity_id.map(str::to_string),
        status: "ACTIVE".into(),
        daily_budget_fen: Fen(daily_budget_fen),
        bid_fen: Fen(bid_fen),
        updated_at,
        raw_extra,
    }
}

/// Seeds full enterprise governance, people/bindings, projects, creative taxonomy,
/// production workload credits, and default custom calibers into `storage`.
pub async fn seed_mock_governance(storage: &dyn StorageService) -> Result<(), StorageError> {
    let org = Organization {
        id: "org-starpulse".into(),
        name: "星图智投数字营销集团 (StarPulse Agency)".into(),
        org_type: OrgType::Agency,
        default_currency: "CNY".into(),
    };
    storage.upsert_organization(&org).await?;

    let people = vec![
        Person {
            id: "per-fde".into(),
            org_id: org.id.clone(),
            name: "顾远 (FDE架构师)".into(),
            email: Some("guyuan@starpulse.example".into()),
            department: "AI与数据底座部".into(),
            role: PersonRole::FdeAdmin,
            monthly_cost_fen: Fen(35_000_00),
            active: true,
        },
        Person {
            id: "per-opt-lin".into(),
            org_id: org.id.clone(),
            name: "林致远".into(),
            email: Some("linzhiyuan@starpulse.example".into()),
            department: "信息流优化一组".into(),
            role: PersonRole::Optimizer,
            monthly_cost_fen: Fen(18_000_00),
            active: true,
        },
        Person {
            id: "per-opt-su".into(),
            org_id: org.id.clone(),
            name: "苏瑾".into(),
            email: Some("sujin@starpulse.example".into()),
            department: "社交种草优化组".into(),
            role: PersonRole::Optimizer,
            monthly_cost_fen: Fen(17_500_00),
            active: true,
        },
        Person {
            id: "per-opt-zhou".into(),
            org_id: org.id.clone(),
            name: "周锐".into(),
            email: Some("zhourui@starpulse.example".into()),
            department: "本地生活与生态组".into(),
            role: PersonRole::Optimizer,
            monthly_cost_fen: Fen(16_500_00),
            active: true,
        },
        Person {
            id: "per-am-wang".into(),
            org_id: org.id.clone(),
            name: "王若曦".into(),
            email: Some("wangruoxi@starpulse.example".into()),
            department: "大客户商务部".into(),
            role: PersonRole::AccountManager,
            monthly_cost_fen: Fen(22_000_00),
            active: true,
        },
        Person {
            id: "per-media-chen".into(),
            org_id: org.id.clone(),
            name: "陈思羽".into(),
            email: Some("chensiyu@starpulse.example".into()),
            department: "达人媒介部".into(),
            role: PersonRole::MediaBuyer,
            monthly_cost_fen: Fen(16_000_00),
            active: true,
        },
        Person {
            id: "per-script-zhao".into(),
            org_id: org.id.clone(),
            name: "赵星河".into(),
            email: Some("zhaoxinghe@starpulse.example".into()),
            department: "创意内容中心".into(),
            role: PersonRole::Copywriter,
            monthly_cost_fen: Fen(15_000_00),
            active: true,
        },
        Person {
            id: "per-shoot-xu".into(),
            org_id: org.id.clone(),
            name: "徐子墨".into(),
            email: Some("xuzimo@starpulse.example".into()),
            department: "创意内容中心".into(),
            role: PersonRole::Videographer,
            monthly_cost_fen: Fen(15_500_00),
            active: true,
        },
        Person {
            id: "per-edit-tang".into(),
            org_id: org.id.clone(),
            name: "唐可欣".into(),
            email: Some("tangkexin@starpulse.example".into()),
            department: "创意内容中心".into(),
            role: PersonRole::Editor,
            monthly_cost_fen: Fen(14_500_00),
            active: true,
        },
        Person {
            id: "per-design-ye".into(),
            org_id: org.id.clone(),
            name: "叶明轩".into(),
            email: Some("yemingxuan@starpulse.example".into()),
            department: "视觉设计组".into(),
            role: PersonRole::Designer,
            monthly_cost_fen: Fen(14_000_00),
            active: true,
        },
        Person {
            id: "per-opt-global".into(),
            org_id: org.id.clone(),
            name: "Alex Chen".into(),
            email: Some("alex.chen@starpulse.example".into()),
            department: "海外DTC增长组".into(),
            role: PersonRole::Optimizer,
            monthly_cost_fen: Fen(21_000_00),
            active: true,
        },
    ];
    for p in &people {
        storage.upsert_person(p).await?;
    }

    let account_specs = [
        (PlatformId::TencentAds, "腾讯广点通_护肤主户", 0.06, 0.08, 0.88),
        (PlatformId::XhsJuguang, "小红书聚光_奢宠护肤户", 0.04, 0.08, 0.92),
        (PlatformId::XhsChengfeng, "小红书乘风_闭环成交户", 0.04, 0.08, 0.90),
        (PlatformId::XhsPugongying, "小红书蒲公英_品牌采买户", 0.00, 0.10, 1.00),
        (PlatformId::BilibiliSanlian, "B站三连起飞_数码核心户", 0.05, 0.08, 0.87),
        (PlatformId::BilibiliHuahuo, "B站花火_UP商单采买户", 0.00, 0.10, 1.00),
        (PlatformId::Kuaishou, "快手磁力引擎_爆品小店户", 0.07, 0.06, 0.84),
        (PlatformId::OceanEngineAd, "巨量引擎AD_全店放量主户", 0.06, 0.07, 0.86),
        (PlatformId::OceanEngineLocal, "巨量本地推_华东门店户", 0.05, 0.08, 0.90),
        (PlatformId::AlipayDenghuo, "支付宝灯火_消费券拉新户", 0.05, 0.07, 0.91),
        (PlatformId::TikTokAds, "TikTok Ads_US_DTC_Main", 0.03, 0.07, 1.00),
        (PlatformId::MetaAds, "Meta Ads_Global_Reels_Primary", 0.02, 0.07, 1.00),
        (PlatformId::GoogleAds, "Google Ads_US_Search_PMax", 0.02, 0.07, 1.00),
        (PlatformId::AppleSearchAds, "Apple Search Ads_US_BeautyApp", 0.00, 0.08, 1.00),
        (PlatformId::AmazonAds, "Amazon Ads_US_HeroSKU_SP", 0.00, 0.08, 1.00),
    ];

    let mut accounts = Vec::new();
    for (p, name, rebate, svc, cash_ratio) in account_specs {
        let extra = match p.default_currency() {
            "USD" => json!({ "mode": "mock", "currency": "USD", "fx_rate_to_base": 7.25 }),
            cur => json!({ "mode": "mock", "currency": cur, "fx_rate_to_base": 1.0 }),
        };
        let acct = AdAccount {
            id: mock_account_id(p).into(),
            org_id: org.id.clone(),
            platform: p,
            platform_account_id: mock_platform_account_id(p).into(),
            account_name: name.into(),
            account_type: AccountType::AgencySub,
            rebate_rate: rebate,
            service_fee_rate: svc,
            default_cash_ratio: cash_ratio,
            status: "active".into(),
            credential_ref: Some(p.as_str().into()),
            extra,
        };
        storage.upsert_ad_account(&acct).await?;
        let ents = mock_entities_for_account(&acct);
        storage.upsert_ad_entities(&ents).await?;
        accounts.push(acct);
    }

    // Effective-dated `人-户关系` including co-optimization splits (60/40) and mid-year handoff!
    let early = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
    let handoff = NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
    let bindings = vec![
        PersonAccountBinding {
            id: "bind-oe-lin-1".into(),
            person_id: "per-opt-lin".into(),
            ad_account_id: mock_account_id(PlatformId::OceanEngineAd).into(),
            relation_role: BindingRole::PrimaryOptimizer,
            split_weight: 0.70,
            effective_from: early,
            effective_to: None,
        },
        PersonAccountBinding {
            id: "bind-oe-zhou-2".into(),
            person_id: "per-opt-zhou".into(),
            ad_account_id: mock_account_id(PlatformId::OceanEngineAd).into(),
            relation_role: BindingRole::CoOptimizer,
            split_weight: 0.30,
            effective_from: handoff,
            effective_to: None,
        },
        PersonAccountBinding {
            id: "bind-tc-lin".into(),
            person_id: "per-opt-lin".into(),
            ad_account_id: mock_account_id(PlatformId::TencentAds).into(),
            relation_role: BindingRole::PrimaryOptimizer,
            split_weight: 1.0,
            effective_from: early,
            effective_to: None,
        },
        PersonAccountBinding {
            id: "bind-ks-lin".into(),
            person_id: "per-opt-lin".into(),
            ad_account_id: mock_account_id(PlatformId::Kuaishou).into(),
            relation_role: BindingRole::PrimaryOptimizer,
            split_weight: 1.0,
            effective_from: early,
            effective_to: None,
        },
        PersonAccountBinding {
            id: "bind-jg-su".into(),
            person_id: "per-opt-su".into(),
            ad_account_id: mock_account_id(PlatformId::XhsJuguang).into(),
            relation_role: BindingRole::PrimaryOptimizer,
            split_weight: 1.0,
            effective_from: early,
            effective_to: None,
        },
        PersonAccountBinding {
            id: "bind-cf-su".into(),
            person_id: "per-opt-su".into(),
            ad_account_id: mock_account_id(PlatformId::XhsChengfeng).into(),
            relation_role: BindingRole::PrimaryOptimizer,
            split_weight: 1.0,
            effective_from: early,
            effective_to: None,
        },
        PersonAccountBinding {
            id: "bind-pgy-chen".into(),
            person_id: "per-media-chen".into(),
            ad_account_id: mock_account_id(PlatformId::XhsPugongying).into(),
            relation_role: BindingRole::MediaBuyer,
            split_weight: 1.0,
            effective_from: early,
            effective_to: None,
        },
        PersonAccountBinding {
            id: "bind-sl-su".into(),
            person_id: "per-opt-su".into(),
            ad_account_id: mock_account_id(PlatformId::BilibiliSanlian).into(),
            relation_role: BindingRole::PrimaryOptimizer,
            split_weight: 0.60,
            effective_from: early,
            effective_to: None,
        },
        PersonAccountBinding {
            id: "bind-sl-zhou".into(),
            person_id: "per-opt-zhou".into(),
            ad_account_id: mock_account_id(PlatformId::BilibiliSanlian).into(),
            relation_role: BindingRole::CoOptimizer,
            split_weight: 0.40,
            effective_from: early,
            effective_to: None,
        },
        PersonAccountBinding {
            id: "bind-hh-chen".into(),
            person_id: "per-media-chen".into(),
            ad_account_id: mock_account_id(PlatformId::BilibiliHuahuo).into(),
            relation_role: BindingRole::MediaBuyer,
            split_weight: 1.0,
            effective_from: early,
            effective_to: None,
        },
        PersonAccountBinding {
            id: "bind-loc-zhou".into(),
            person_id: "per-opt-zhou".into(),
            ad_account_id: mock_account_id(PlatformId::OceanEngineLocal).into(),
            relation_role: BindingRole::PrimaryOptimizer,
            split_weight: 1.0,
            effective_from: early,
            effective_to: None,
        },
        PersonAccountBinding {
            id: "bind-dh-zhou".into(),
            person_id: "per-opt-zhou".into(),
            ad_account_id: mock_account_id(PlatformId::AlipayDenghuo).into(),
            relation_role: BindingRole::PrimaryOptimizer,
            split_weight: 1.0,
            effective_from: early,
            effective_to: None,
        },
        PersonAccountBinding {
            id: "bind-tt-global".into(),
            person_id: "per-opt-global".into(),
            ad_account_id: mock_account_id(PlatformId::TikTokAds).into(),
            relation_role: BindingRole::PrimaryOptimizer,
            split_weight: 1.0,
            effective_from: early,
            effective_to: None,
        },
        PersonAccountBinding {
            id: "bind-fb-global".into(),
            person_id: "per-opt-global".into(),
            ad_account_id: mock_account_id(PlatformId::MetaAds).into(),
            relation_role: BindingRole::PrimaryOptimizer,
            split_weight: 1.0,
            effective_from: early,
            effective_to: None,
        },
        PersonAccountBinding {
            id: "bind-gg-global".into(),
            person_id: "per-opt-global".into(),
            ad_account_id: mock_account_id(PlatformId::GoogleAds).into(),
            relation_role: BindingRole::PrimaryOptimizer,
            split_weight: 1.0,
            effective_from: early,
            effective_to: None,
        },
        PersonAccountBinding {
            id: "bind-asa-global".into(),
            person_id: "per-opt-global".into(),
            ad_account_id: mock_account_id(PlatformId::AppleSearchAds).into(),
            relation_role: BindingRole::PrimaryOptimizer,
            split_weight: 1.0,
            effective_from: early,
            effective_to: None,
        },
        PersonAccountBinding {
            id: "bind-amz-global".into(),
            person_id: "per-opt-global".into(),
            ad_account_id: mock_account_id(PlatformId::AmazonAds).into(),
            relation_role: BindingRole::PrimaryOptimizer,
            split_weight: 1.0,
            effective_from: early,
            effective_to: None,
        },
    ];
    for b in &bindings {
        storage.upsert_person_account_binding(b).await?;
    }

    // 6 Projects covering domestic & overseas `ProjectType` categories.
    let projects = vec![
        Project {
            id: "prj-ecom-skincare".into(),
            org_id: org.id.clone(),
            code: "PRJ-ECOM-01".into(),
            name: "润光肌秋冬电商大促成交".into(),
            client_name: "润光生物科技".into(),
            brand_name: "润光肌 GlowLab".into(),
            project_type: ProjectType::EcommerceGmv,
            business_line: "美妆护肤".into(),
            settlement_mode: SettlementMode::MediaSpendPlusFee,
            budget_fen: Fen(2_500_000_00),
            target_roi: Some(3.2),
            target_cpa_fen: None,
            owner_person_id: Some("per-am-wang".into()),
            start_date: early,
            end_date: None,
            tags: vec!["双11".into(), "GMV".into(), "全站放量".into()],
        },
        Project {
            id: "prj-seed-xhs".into(),
            org_id: org.id.clone(),
            code: "PRJ-SEED-02".into(),
            name: "小红书蒲公英+聚光乘风全链路种草".into(),
            client_name: "润光生物科技".into(),
            brand_name: "润光肌 GlowLab".into(),
            project_type: ProjectType::SeedingContent,
            business_line: "美妆护肤".into(),
            settlement_mode: SettlementMode::IntegratedPackage,
            budget_fen: Fen(1_200_000_00),
            target_roi: Some(2.6),
            target_cpa_fen: None,
            owner_person_id: Some("per-opt-su".into()),
            start_date: early,
            end_date: None,
            tags: vec!["种草闭环".into(), "蒲公英+聚光".into()],
        },
        Project {
            id: "prj-bili-3c".into(),
            org_id: org.id.clone(),
            code: "PRJ-BRAND-03".into(),
            name: "B站花火UP商单+三连起飞心智种草".into(),
            client_name: "极光声学科技".into(),
            brand_name: "Aurora Audio".into(),
            project_type: ProjectType::BrandAwareness,
            business_line: "3C消费电子".into(),
            settlement_mode: SettlementMode::IntegratedPackage,
            budget_fen: Fen(950_000_00),
            target_roi: Some(2.4),
            target_cpa_fen: None,
            owner_person_id: Some("per-media-chen".into()),
            start_date: early,
            end_date: None,
            tags: vec!["B站花火".into(), "商业起飞".into()],
        },
        Project {
            id: "prj-local-tea".into(),
            org_id: org.id.clone(),
            code: "PRJ-LOCAL-04".into(),
            name: "华东20城茶饮团购本地推核销".into(),
            client_name: "云山茶事".into(),
            brand_name: "云山茶事".into(),
            project_type: ProjectType::LocalLife,
            business_line: "本地生活餐饮".into(),
            settlement_mode: SettlementMode::PerformanceCpaCps,
            budget_fen: Fen(700_000_00),
            target_roi: Some(3.8),
            target_cpa_fen: Some(Fen(18_00)),
            owner_person_id: Some("per-opt-zhou".into()),
            start_date: early,
            end_date: None,
            tags: vec!["本地推".into(), "POI团购".into()],
        },
        Project {
            id: "prj-perf-fintech".into(),
            org_id: org.id.clone(),
            code: "PRJ-PERF-05".into(),
            name: "支付宝灯火消费券小程序拉新".into(),
            client_name: "悦享零售".into(),
            brand_name: "悦享严选".into(),
            project_type: ProjectType::PerformanceAcquisition,
            business_line: "零售会员拉新".into(),
            settlement_mode: SettlementMode::PerformanceCpaCps,
            budget_fen: Fen(600_000_00),
            target_roi: Some(3.0),
            target_cpa_fen: Some(Fen(22_00)),
            owner_person_id: Some("per-opt-zhou".into()),
            start_date: early,
            end_date: None,
            tags: vec!["支付宝灯火".into(), "小程序激活".into()],
        },
        Project {
            id: "prj-global-dtc".into(),
            org_id: org.id.clone(),
            code: "PRJ-GLOBAL-06".into(),
            name: "GlowLab 欧美DTC全渠道出海增长".into(),
            client_name: "润光生物科技".into(),
            brand_name: "GlowLab Global".into(),
            project_type: ProjectType::EcommerceGmv,
            business_line: "海外DTC独立站+Amazon+App".into(),
            settlement_mode: SettlementMode::MediaSpendPlusFee,
            budget_fen: Fen(3_200_000_00),
            target_roi: Some(3.1),
            target_cpa_fen: Some(Fen(120_00)),
            owner_person_id: Some("per-opt-global".into()),
            start_date: early,
            end_date: None,
            tags: vec!["出海DTC".into(), "USD折算CNY".into(), "多平台归因".into()],
        },
    ];
    for p in &projects {
        storage.upsert_project(p).await?;
    }

    let proj_links = vec![
        ("pal-1", "prj-ecom-skincare", PlatformId::OceanEngineAd),
        ("pal-2", "prj-ecom-skincare", PlatformId::TencentAds),
        ("pal-3", "prj-ecom-skincare", PlatformId::Kuaishou),
        ("pal-4", "prj-seed-xhs", PlatformId::XhsJuguang),
        ("pal-5", "prj-seed-xhs", PlatformId::XhsChengfeng),
        ("pal-6", "prj-seed-xhs", PlatformId::XhsPugongying),
        ("pal-7", "prj-bili-3c", PlatformId::BilibiliSanlian),
        ("pal-8", "prj-bili-3c", PlatformId::BilibiliHuahuo),
        ("pal-9", "prj-local-tea", PlatformId::OceanEngineLocal),
        ("pal-10", "prj-perf-fintech", PlatformId::AlipayDenghuo),
        ("pal-11", "prj-global-dtc", PlatformId::TikTokAds),
        ("pal-12", "prj-global-dtc", PlatformId::MetaAds),
        ("pal-13", "prj-global-dtc", PlatformId::GoogleAds),
        ("pal-14", "prj-global-dtc", PlatformId::AppleSearchAds),
        ("pal-15", "prj-global-dtc", PlatformId::AmazonAds),
    ];
    for (id, pid, plat) in proj_links {
        storage
            .upsert_project_account_link(&ProjectAccountLink {
                id: id.into(),
                project_id: pid.into(),
                ad_account_id: mock_account_id(plat).into(),
                campaign_filter_regex: None,
                allocation_weight: 1.0,
            })
            .await?;
    }

    // Creative Assets + Cross-platform links + Internal Creator Workload Credits.
    let created = NaiveDate::from_ymd_opt(2026, 5, 1).unwrap();
    let creative_specs = vec![
        (
            "cr-oe-01",
            "prj-ecom-skincare",
            "CR-OE-001",
            "3秒痛点开场+实验室精华实测",
            CreativeFormat::ShortVideo,
            CreativeHookType::PainPoint,
            "深夜护肤场景",
            "28天淡纹数据",
            "硬核实测",
            2800_00,
            18.0,
            PlatformId::OceanEngineAd,
            "oe_cre_01",
            Some("mat_oe_901"),
        ),
        (
            "cr-oe-02",
            "prj-ecom-skincare",
            "CR-OE-002",
            "创始人故事+周年礼盒开箱",
            CreativeFormat::ShortVideo,
            CreativeHookType::Story,
            "研发工作室",
            "礼盒加赠正装",
            "温情信任",
            3200_00,
            22.0,
            PlatformId::OceanEngineAd,
            "oe_cre_02",
            Some("mat_oe_902"),
        ),
        (
            "cr-tc-01",
            "prj-ecom-skincare",
            "CR-TC-003",
            "朋友圈实验室成分拆解竖版",
            CreativeFormat::ShortVideo,
            CreativeHookType::ExpertAuthority,
            "成分科普实验室",
            "烟酰胺微囊渗透",
            "专业权威",
            2400_00,
            16.0,
            PlatformId::TencentAds,
            "tc_cre_01",
            Some("mat_tc_01"),
        ),
        (
            "cr-ks-01",
            "prj-ecom-skincare",
            "CR-KS-004",
            "快手老铁源头工厂溯源短视频",
            CreativeFormat::LiveClip,
            CreativeHookType::PromoPrice,
            "GMP生产车间",
            "破价买一送一",
            "接地气爽快",
            1900_00,
            14.0,
            PlatformId::Kuaishou,
            "ks_cre_01",
            Some("ks_photo_801"),
        ),
        (
            "cr-xhs-101",
            "prj-seed-xhs",
            "CR-XHS-101",
            "小红书爆文Note101_熬夜党早C晚A跟拍(蒲公英+聚光+乘风)",
            CreativeFormat::ImageTextNote,
            CreativeHookType::Contrast,
            "早八通勤化妆台",
            "14天透亮不暗沉",
            "真实种草",
            2200_00,
            15.0,
            PlatformId::XhsJuguang,
            "jg_cre_01",
            Some("note_xhs_101"),
        ),
        (
            "cr-xhs-102",
            "prj-seed-xhs",
            "CR-XHS-102",
            "小红书图文测评_成分党拆解",
            CreativeFormat::ImageTextNote,
            CreativeHookType::ExpertAuthority,
            "居家书桌评测",
            "0酒精温和修护",
            "理性种草",
            1500_00,
            12.0,
            PlatformId::XhsJuguang,
            "jg_cre_02",
            Some("note_xhs_102"),
        ),
        (
            "cr-bili-01",
            "prj-bili-3c",
            "CR-BILI-01",
            "B站花火+三连起飞_百大科技UP深度拆机",
            CreativeFormat::KolCustom,
            CreativeHookType::Unboxing,
            "极客评测工作室",
            "主动降噪48dB+空间音频",
            "硬核极客",
            3600_00,
            24.0,
            PlatformId::BilibiliSanlian,
            "bl_cre_01",
            Some("BV1xx411c7mD"),
        ),
        (
            "cr-loc-01",
            "prj-local-tea",
            "CR-LOC-01",
            "本地推探店Vlog_南京西路招牌鲜果茶套餐",
            CreativeFormat::ShortVideo,
            CreativeHookType::Lifestyle,
            "城市门店探店",
            "双人套餐5.8折核销",
            "轻快探店",
            1800_00,
            12.0,
            PlatformId::OceanEngineLocal,
            "loc_cre_01",
            Some("poi_sh_001"),
        ),
        (
            "cr-dh-01",
            "prj-perf-fintech",
            "CR-DH-01",
            "灯火支付立减5元权益卡片设计",
            CreativeFormat::Carousel,
            CreativeHookType::PromoPrice,
            "支付宝支付成功页",
            "领券立减5元",
            "简洁直观",
            900_00,
            8.0,
            PlatformId::AlipayDenghuo,
            "dh_cre_01",
            Some("mat_dh_01"),
        ),
        (
            "cr-tt-01",
            "prj-global-dtc",
            "CR-TT-01",
            "TikTok US UGC Before/After Glow Serum 15s",
            CreativeFormat::ShortVideo,
            CreativeHookType::Contrast,
            "Morning Vanity Bathroom",
            "7-Day Radiance Clinical Boost",
            "Authentic Creator UGC",
            3200_00,
            16.0,
            PlatformId::TikTokAds,
            "tt_ad_01",
            Some("v_tt_001"),
        ),
        (
            "cr-fb-01",
            "prj-global-dtc",
            "CR-FB-01",
            "Meta Reels Founder Formulation Breakdown",
            CreativeFormat::ShortVideo,
            CreativeHookType::Story,
            "Lab & Studio",
            "10% Encapsulated Peptide Complex",
            "Premium Editorial",
            2900_00,
            14.0,
            PlatformId::MetaAds,
            "fb_ad_01",
            Some("mat_fb_01"),
        ),
        (
            "cr-gg-01",
            "prj-global-dtc",
            "CR-GG-01",
            "YouTube Shorts Clinical Peptide Trial 30s",
            CreativeFormat::ShortVideo,
            CreativeHookType::ExpertAuthority,
            "Dermatology Lab",
            "94% Smoother Skin in 28 Days",
            "Science-First Authority",
            2600_00,
            12.0,
            PlatformId::GoogleAds,
            "gg_ad_01",
            Some("mat_gg_01"),
        ),
    ];

    for (
        idx,
        (
            id,
            proj_id,
            code,
            title,
            fmt,
            hook,
            scene,
            selling_pt,
            tone,
            prod_cost,
            hours,
            plat,
            cre_id,
            content_id,
        ),
    ) in creative_specs.iter().enumerate()
    {
        storage
            .upsert_creative_asset(&CreativeAsset {
                id: (*id).into(),
                project_id: (*proj_id).into(),
                asset_code: (*code).into(),
                title: (*title).into(),
                taxonomy: CreativeTaxonomy {
                    format: fmt.clone(),
                    hook_type: hook.clone(),
                    scene_tag: (*scene).into(),
                    selling_point: (*selling_pt).into(),
                    tone: (*tone).into(),
                    custom_tags: BTreeMap::from([("q_batch".into(), format!("B{:02}", idx + 1))]),
                },
                production_cost_fen: Fen(*prod_cost),
                production_hours: *hours,
                created_date: created,
                status: "active".into(),
            })
            .await?;

        storage
            .upsert_creative_platform_link(&CreativePlatformLink {
                id: format!("cpl-{idx}-main"),
                asset_id: (*id).into(),
                platform: *plat,
                ad_account_id: mock_account_id(*plat).into(),
                platform_creative_id: (*cre_id).into(),
                platform_content_id: content_id.map(str::to_string),
            })
            .await?;

        // Split internal production workload across Scriptwriter, Shooter, Editor, Designer.
        let contributions = [
            ("per-script-zhao", ProductionRole::Scriptwriter, hours * 0.25, 0.30),
            ("per-shoot-xu", ProductionRole::Shooter, hours * 0.30, 0.25),
            ("per-edit-tang", ProductionRole::Editor, hours * 0.35, 0.30),
            ("per-design-ye", ProductionRole::Designer, hours * 0.10, 0.15),
        ];
        for (cidx, (person_id, role, wh, cw)) in contributions.iter().enumerate() {
            storage
                .upsert_creative_contribution(&CreativeProductionContribution {
                    id: format!("cpc-{idx}-{cidx}"),
                    asset_id: (*id).into(),
                    person_id: (*person_id).into(),
                    role: role.clone(),
                    work_hours: (*wh * 10.0).round() / 10.0,
                    credit_weight: *cw,
                    piece_cost_fen: Fen(*prod_cost).scale(*cw),
                })
                .await?;
        }
    }

    // Cross-link Pugongying order + Chengfeng harvest + Huahuo order onto the shared creative assets!
    storage
        .upsert_creative_platform_link(&CreativePlatformLink {
            id: "cpl-xhs-pgy-101".into(),
            asset_id: "cr-xhs-101".into(),
            platform: PlatformId::XhsPugongying,
            ad_account_id: mock_account_id(PlatformId::XhsPugongying).into(),
            platform_creative_id: "pgy_ord_101".into(),
            platform_content_id: Some("note_xhs_101".into()),
        })
        .await?;
    storage
        .upsert_creative_platform_link(&CreativePlatformLink {
            id: "cpl-xhs-cf-101".into(),
            asset_id: "cr-xhs-101".into(),
            platform: PlatformId::XhsChengfeng,
            ad_account_id: mock_account_id(PlatformId::XhsChengfeng).into(),
            platform_creative_id: "cf_cre_01".into(),
            platform_content_id: Some("note_xhs_101".into()),
        })
        .await?;
    storage
        .upsert_creative_platform_link(&CreativePlatformLink {
            id: "cpl-bili-hh-01".into(),
            asset_id: "cr-bili-01".into(),
            platform: PlatformId::BilibiliHuahuo,
            ad_account_id: mock_account_id(PlatformId::BilibiliHuahuo).into(),
            platform_creative_id: "hh_ord_01".into(),
            platform_content_id: Some("BV1xx411c7mD".into()),
        })
        .await?;

    // 4 default Custom Calibers showcasing the safe AST formula DSL.
    let calibers = vec![
        CustomCaliberDef {
            id: "cal-effective-net-roi".into(),
            name: "effective_net_roi".into(),
            display_name: "品牌净投入ROI (扣除返点+含商单)".into(),
            category: CaliberCategory::Performance,
            formula: "safe_div(gmv_yuan, net_after_rebate_yuan + kol_order_fee_yuan + platform_service_fee_yuan)".into(),
            unit: "ratio".into(),
            description: "综合返点折让、达人商单及平台服务费后的真实净ROI".into(),
            enabled: true,
        },
        CustomCaliberDef {
            id: "cal-agency-gross-margin".into(),
            name: "agency_gross_profit_yuan".into(),
            display_name: "代理商综合毛利(元)".into(),
            category: CaliberCategory::Performance,
            formula: "agency_net_revenue_yuan - production_cost_yuan".into(),
            unit: "yuan".into(),
            description: "代理商返点+服务费净营收扣除内部素材创作直接成本".into(),
            enabled: true,
        },
        CustomCaliberDef {
            id: "cal-full-funnel-cpe".into(),
            name: "full_funnel_cpe_yuan".into(),
            display_name: "全域互动成本CPE(含种草商单)".into(),
            category: CaliberCategory::Efficiency,
            formula: "safe_div(gross_cost_yuan + kol_order_fee_yuan + platform_service_fee_yuan, interactions)".into(),
            unit: "yuan".into(),
            description: "媒体加热消耗与达人商单打包后的单次互动成本".into(),
            enabled: true,
        },
        CustomCaliberDef {
            id: "cal-cash-share-pct".into(),
            name: "cash_consumption_pct".into(),
            display_name: "现金消耗占比(%)".into(),
            category: CaliberCategory::Spend,
            formula: "safe_div(cash_cost_yuan * 100, gross_cost_yuan)".into(),
            unit: "percent".into(),
            description: "现金实际扣款占媒体账面总消耗的百分比".into(),
            enabled: true,
        },
    ];
    for c in &calibers {
        storage.upsert_custom_caliber(c).await?;
    }

    Ok(())
}
