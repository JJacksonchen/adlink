//! Official-schema metric sample synthesis so every connector's mock mode emits realistic,
//! deterministic official JSON payloads and exercises the live response parser.

use adlink_domain::{AdAccount, AdEntity, Fen, PlatformId, ReportGranularity, ReportLevel};
use chrono::NaiveDate;

use crate::mock_data::{mock_entities_for_account, seasonal_factor, stable_seed, DeterministicRng};

pub struct SyntheticDayMetric {
    pub entity_id: String,
    pub entity_name: String,
    pub parent_entity_id: Option<String>,
    pub content_id: Option<String>,
    pub stat_hour: i32,
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
}

/// Generates deterministic realistic metrics for `account` at `level` across `[start_date, end_date]`
/// with pagination support (`page`, `page_size`).
pub fn synthesize_chunk_metrics(
    account: &AdAccount,
    start_date: NaiveDate,
    end_date: NaiveDate,
    granularity: ReportGranularity,
    level: ReportLevel,
    page: u32,
    page_size: u32,
) -> (Vec<(NaiveDate, SyntheticDayMetric)>, bool, u64) {
    let all_entities = mock_entities_for_account(account);
    let mut scoped: Vec<AdEntity> = all_entities
        .into_iter()
        .filter(|e| e.level == level)
        .collect();

    // If a platform (e.g. PGY/Huahuo or Account rollup) has no direct rows at `level`, synthesize
    // an account-level summary entity so cross-platform sync jobs always behave uniformly.
    if scoped.is_empty() {
        scoped.push(AdEntity {
            platform: account.platform,
            ad_account_id: account.id.clone(),
            level,
            entity_id: format!("{}_{}", account.platform_account_id, level.as_str()),
            entity_name: account.account_name.clone(),
            parent_entity_id: None,
            status: "ACTIVE".into(),
            daily_budget_fen: Fen(200_000),
            bid_fen: Fen(35_00),
            updated_at: chrono::Utc::now(),
            raw_extra: serde_json::Value::Null,
        });
    }

    let mut all_items = Vec::new();
    let mut cur = start_date;
    while cur <= end_date {
        let hours: &[i32] = if granularity == ReportGranularity::Hourly {
            &[9, 12, 18, 21]
        } else {
            &[-1]
        };
        for &hr in hours {
            for (idx, ent) in scoped.iter().enumerate() {
                let m = build_single_metric(account, ent, cur, hr, idx);
                all_items.push((cur, m));
            }
        }
        cur = match cur.succ_opt() {
            Some(n) => n,
            None => break,
        };
    }

    let total = all_items.len() as u64;
    let start_idx = ((page.saturating_sub(1)) as usize) * (page_size as usize);
    let end_idx = (start_idx + page_size as usize).min(all_items.len());
    let has_more = end_idx < all_items.len();
    let slice = if start_idx < all_items.len() {
        all_items.drain(start_idx..end_idx).collect()
    } else {
        Vec::new()
    };

    (slice, has_more, total)
}

fn build_single_metric(
    account: &AdAccount,
    ent: &AdEntity,
    date: NaiveDate,
    stat_hour: i32,
    idx: usize,
) -> SyntheticDayMetric {
    let seed = stable_seed(
        &format!("{}:{}:{}", account.platform, ent.entity_id, stat_hour),
        date,
        idx,
    );
    let mut rng = DeterministicRng::from_seed(seed);
    let season = seasonal_factor(date);
    let hour_scale = if stat_hour >= 0 { 0.25 } else { 1.0 };

    let content_id = ent
        .raw_extra
        .get("note_id")
        .or_else(|| ent.raw_extra.get("bvid"))
        .or_else(|| ent.raw_extra.get("photo_id"))
        .or_else(|| ent.raw_extra.get("poi_id"))
        .or_else(|| ent.raw_extra.get("material_id"))
        .or_else(|| ent.raw_extra.get("content_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);

    let is_kol_seeding = matches!(
        account.platform,
        PlatformId::XhsPugongying | PlatformId::BilibiliHuahuo
    );

    if is_kol_seeding {
        // KOL seeding orders occur primarily on publish/settlement dates or amortized daily bursts.
        let base_order_yuan = match account.platform {
            PlatformId::BilibiliHuahuo => 1800.0,
            _ => 950.0,
        };
        let kol_fee_fen =
            Fen::from_yuan_f64(base_order_yuan * rng.range_f64(0.85, 1.25) * season * hour_scale);
        let svc_fee_fen = kol_fee_fen.scale(account.service_fee_rate);
        let impressions = (rng.range_f64(18_000.0, 42_000.0) * season * hour_scale).round() as u64;
        let clicks = (impressions as f64 * rng.range_f64(0.032, 0.055)).round() as u64;
        let interactions = (impressions as f64 * rng.range_f64(0.028, 0.048)).round() as u64;
        let conversions = (clicks as f64 * rng.range_f64(0.025, 0.045)).round() as u64;
        let gmv_fen = Fen::from_yuan_f64(
            (kol_fee_fen + svc_fee_fen).to_yuan_f64() * rng.range_f64(1.6, 2.4),
        );

        return SyntheticDayMetric {
            entity_id: ent.entity_id.clone(),
            entity_name: ent.entity_name.clone(),
            parent_entity_id: ent.parent_entity_id.clone(),
            content_id,
            stat_hour,
            gross_cost_fen: Fen::ZERO,
            cash_cost_fen: Fen::ZERO,
            grant_cost_fen: Fen::ZERO,
            kol_order_fee_fen: kol_fee_fen,
            platform_service_fee_fen: svc_fee_fen,
            impressions,
            clicks,
            conversions,
            deep_conversions: conversions / 4,
            leads: conversions / 5,
            orders: (conversions as f64 * 0.85).round() as u64,
            gmv_fen,
            interactions,
            video_plays: if account.platform == PlatformId::BilibiliHuahuo {
                (impressions as f64 * 0.65).round() as u64
            } else {
                0
            },
            effective_plays: if account.platform == PlatformId::BilibiliHuahuo {
                (impressions as f64 * 0.42).round() as u64
            } else {
                0
            },
            new_follows: interactions / 6,
            store_verifies: 0,
        };
    }

    let base_spend_yuan = match account.platform {
        PlatformId::OceanEngineAd => 6800.0,
        PlatformId::TencentAds => 4900.0,
        PlatformId::Kuaishou => 4200.0,
        PlatformId::XhsChengfeng => 3100.0,
        PlatformId::XhsJuguang => 2600.0,
        PlatformId::BilibiliSanlian => 2200.0,
        PlatformId::OceanEngineLocal => 2500.0,
        PlatformId::AlipayDenghuo => 2400.0,
        PlatformId::MetaAds => 5400.0,
        PlatformId::GoogleAds => 5100.0,
        PlatformId::TikTokAds => 4600.0,
        PlatformId::AmazonAds => 3900.0,
        PlatformId::AppleSearchAds => 2900.0,
        _ => 2000.0,
    };

    let level_divisor = match ent.level {
        ReportLevel::Account => 1.0,
        ReportLevel::Campaign => 1.0,
        ReportLevel::AdGroup => 1.0,
        ReportLevel::Creative => 0.55,
        ReportLevel::ContentOrder => 0.55,
    };

    let gross_yuan = base_spend_yuan * level_divisor * season * hour_scale * rng.range_f64(0.82, 1.22);
    let gross_cost_fen = Fen::from_yuan_f64(gross_yuan);
    let cash_cost_fen = gross_cost_fen.scale(account.default_cash_ratio);
    let grant_cost_fen = gross_cost_fen - cash_cost_fen;

    let cpm_yuan = rng.range_f64(18.0, 34.0);
    let impressions = ((gross_yuan / cpm_yuan) * 1000.0).round().max(100.0) as u64;
    let ctr = rng.range_f64(0.022, 0.048);
    let clicks = (impressions as f64 * ctr).round().max(1.0) as u64;
    let cvr = rng.range_f64(0.024, 0.052);
    let conversions = (clicks as f64 * cvr).round().max(1.0) as u64;
    let roi_mult = match account.platform {
        PlatformId::OceanEngineLocal => rng.range_f64(3.4, 4.6),
        PlatformId::XhsChengfeng | PlatformId::OceanEngineAd => rng.range_f64(2.9, 4.1),
        PlatformId::AlipayDenghuo => rng.range_f64(2.7, 3.6),
        PlatformId::Kuaishou | PlatformId::TencentAds => rng.range_f64(2.6, 3.5),
        _ => rng.range_f64(2.1, 3.1),
    };
    let gmv_fen = Fen::from_yuan_f64(gross_yuan * roi_mult);
    let interactions = (clicks as f64 * rng.range_f64(0.28, 0.65)).round() as u64;
    let video_plays = (impressions as f64 * rng.range_f64(0.45, 0.72)).round() as u64;
    let effective_plays = (video_plays as f64 * rng.range_f64(0.35, 0.58)).round() as u64;
    let store_verifies = if account.platform == PlatformId::OceanEngineLocal {
        (conversions as f64 * 0.72).round() as u64
    } else {
        0
    };

    SyntheticDayMetric {
        entity_id: ent.entity_id.clone(),
        entity_name: ent.entity_name.clone(),
        parent_entity_id: ent.parent_entity_id.clone(),
        content_id,
        stat_hour,
        gross_cost_fen,
        cash_cost_fen,
        grant_cost_fen,
        kol_order_fee_fen: Fen::ZERO,
        platform_service_fee_fen: Fen::ZERO,
        impressions,
        clicks,
        conversions,
        deep_conversions: (conversions as f64 * 0.35).round() as u64,
        leads: if matches!(
            account.platform,
            PlatformId::TencentAds | PlatformId::AlipayDenghuo
        ) {
            (conversions as f64 * 0.45).round() as u64
        } else {
            0
        },
        orders: (conversions as f64 * 0.88).round() as u64,
        gmv_fen,
        interactions,
        video_plays,
        effective_plays,
        new_follows: interactions / 8,
        store_verifies,
    }
}
