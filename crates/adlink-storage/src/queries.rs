//! Governance, analytics caliber reconciliation, and creative/creator attribution query helpers.

use std::collections::{BTreeMap, HashMap, HashSet};

use adlink_domain::{
    AdAccount, CaliberEvaluator, CaliberInput, CaliberReconciliation, CreativeAsset,
    CreativeAttributionRow, CreativeFormat, CreativeHookType, CreativePlatformLink,
    CreativeProductionContribution, CreatorWorkloadSummary, CustomCaliberDef, Fen, Person,
    PersonAccountBinding, PlatformId, Project, ProjectAccountLink, ProjectType, QueryDimension,
    ReportGranularity, ReportLevel, ReportQuery, SeedingBoostAttributionRow, TimeLine,
    UnifiedReportRow,
};
use chrono::NaiveDate;
use regex::Regex;

use crate::StorageError;

/// Context loaded from storage to perform fast in-memory attribution & caliber evaluation
/// across governance boundaries without fragile SQL dynamic string interpolation.
pub struct AttributionUniverse {
    pub accounts: HashMap<String, AdAccount>,
    pub people: HashMap<String, Person>,
    pub bindings: Vec<PersonAccountBinding>,
    pub projects: HashMap<String, Project>,
    pub project_links: Vec<ProjectAccountLink>,
    pub custom_calibers: Vec<CustomCaliberDef>,
    /// Actual daily cash/(cash+grant) ratios derived from `daily_fund_statements`.
    pub fund_cash_ratios: HashMap<(String, NaiveDate), f64>,
}

/// Groups and evaluates unified report rows across governance dimensions & calibers.
pub fn evaluate_report_query(
    rows: &[UnifiedReportRow],
    query: &ReportQuery,
    universe: &AttributionUniverse,
) -> Result<(CaliberReconciliation, Vec<CaliberReconciliation>), StorageError> {
    let project_filter: HashSet<&String> = query.project_ids.iter().collect();
    let person_filter: HashSet<&String> = query.person_ids.iter().collect();

    // Index account -> project links for O(1) lookup.
    let mut acct_to_projects: HashMap<&str, Vec<&ProjectAccountLink>> = HashMap::new();
    for link in &universe.project_links {
        acct_to_projects
            .entry(link.ad_account_id.as_str())
            .or_default()
            .push(link);
    }

    // Index account -> bindings for O(1) lookup.
    let mut acct_to_bindings: HashMap<&str, Vec<&PersonAccountBinding>> = HashMap::new();
    for b in &universe.bindings {
        acct_to_bindings
            .entry(b.ad_account_id.as_str())
            .or_default()
            .push(b);
    }

    let group_dims: Vec<QueryDimension> = if query.group_by.is_empty() {
        vec![QueryDimension::Platform]
    } else {
        query.group_by.clone()
    };

    // Precompile optional campaign regex filters once per query.
    let compiled_regexes: HashMap<&str, Regex> = universe
        .project_links
        .iter()
        .filter_map(|l| {
            l.campaign_filter_regex
                .as_deref()
                .filter(|p| !p.trim().is_empty())
                .and_then(|pat| Regex::new(pat).ok().map(|re| (l.id.as_str(), re)))
        })
        .collect();

    let mut total_input = CaliberInput::default();
    let mut buckets: BTreeMap<String, CaliberInput> = BTreeMap::new();

    // Prevent double-counting overlapping report hierarchy levels (Account + Campaign + Creative)
    // AND prevent Daily + Hourly or ReportingTime + RequestTime double-counting.
    let level_filtered: Vec<&UnifiedReportRow> = if query.levels.is_empty() {
        select_canonical_level_rows(rows, &group_dims)
    } else {
        rows.iter().collect()
    };
    let selected_rows = dedup_granularity_and_timeline(&level_filtered);

    for row in selected_rows {
        // Apply project link matching including optional `campaign_filter_regex`.
        let matching_proj_links: Vec<&ProjectAccountLink> = acct_to_projects
            .get(row.ad_account_id.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .copied()
            .filter(|l| {
                if row.level == ReportLevel::Account {
                    return true;
                }
                compiled_regexes
                    .get(l.id.as_str())
                    .map_or(true, |re| re.is_match(&row.entity_name))
            })
            .collect();

        if !project_filter.is_empty()
            && !matching_proj_links
                .iter()
                .any(|l| project_filter.contains(&l.project_id))
        {
            continue;
        }

        let fund_ratio_override = universe
            .fund_cash_ratios
            .get(&(row.ad_account_id.clone(), row.stat_date))
            .copied();

        // Resolve active person bindings on row.stat_date.
        let active_bindings: Vec<&PersonAccountBinding> = acct_to_bindings
            .get(row.ad_account_id.as_str())
            .map(|list| {
                list.iter()
                    .copied()
                    .filter(|b| b.is_active_on(row.stat_date))
                    .collect()
            })
            .unwrap_or_default();

        if !person_filter.is_empty()
            && !active_bindings
                .iter()
                .any(|b| person_filter.contains(&b.person_id))
        {
            continue;
        }

        let acct = universe.accounts.get(&row.ad_account_id);
        let rebate_rate = acct.map_or(0.0, |a| a.rebate_rate);
        let svc_rate = acct.map_or(0.0, |a| a.service_fee_rate);

        // If grouping by Person, split metrics strictly by active `split_weight`.
        if group_dims.contains(&QueryDimension::Person) {
            let mut matched_any_person = false;
            for b in &active_bindings {
                if !person_filter.is_empty() && !person_filter.contains(&b.person_id) {
                    continue;
                }
                matched_any_person = true;
                let person_name = universe
                    .people
                    .get(&b.person_id)
                    .map(|p| p.name.as_str())
                    .unwrap_or(b.person_id.as_str());
                let key = build_group_key(
                    row,
                    &group_dims,
                    acct,
                    matching_proj_links.first().copied(),
                    &universe.projects,
                    Some((&b.person_id, person_name)),
                );
                accumulate_row(
                    &mut total_input,
                    row,
                    b.split_weight,
                    rebate_rate,
                    svc_rate,
                    fund_ratio_override,
                );
                accumulate_row(
                    buckets.entry(key).or_default(),
                    row,
                    b.split_weight,
                    rebate_rate,
                    svc_rate,
                    fund_ratio_override,
                );
            }
            if !matched_any_person && person_filter.is_empty() {
                let key = build_group_key(
                    row,
                    &group_dims,
                    acct,
                    matching_proj_links.first().copied(),
                    &universe.projects,
                    Some(("unassigned", "未分配优化师")),
                );
                accumulate_row(
                    &mut total_input,
                    row,
                    1.0,
                    rebate_rate,
                    svc_rate,
                    fund_ratio_override,
                );
                accumulate_row(
                    buckets.entry(key).or_default(),
                    row,
                    1.0,
                    rebate_rate,
                    svc_rate,
                    fund_ratio_override,
                );
            }
            continue;
        }

        // If person_filter is set while grouping by non-person dimensions, scale by matched weights.
        let person_weight = if person_filter.is_empty() {
            1.0
        } else {
            active_bindings
                .iter()
                .filter(|b| person_filter.contains(&b.person_id))
                .map(|b| b.split_weight)
                .sum()
        };

        // If grouping by Project or ProjectType, allocate by `allocation_weight`.
        if group_dims.contains(&QueryDimension::Project)
            || group_dims.contains(&QueryDimension::ProjectType)
        {
            if matching_proj_links.is_empty() {
                let key = build_group_key(
                    row,
                    &group_dims,
                    acct,
                    None,
                    &universe.projects,
                    None,
                );
                accumulate_row(
                    &mut total_input,
                    row,
                    person_weight,
                    rebate_rate,
                    svc_rate,
                    fund_ratio_override,
                );
                accumulate_row(
                    buckets.entry(key).or_default(),
                    row,
                    person_weight,
                    rebate_rate,
                    svc_rate,
                    fund_ratio_override,
                );
            } else {
                for link in matching_proj_links {
                    if !project_filter.is_empty() && !project_filter.contains(&link.project_id) {
                        continue;
                    }
                    let w = link.allocation_weight * person_weight;
                    let key = build_group_key(
                        row,
                        &group_dims,
                        acct,
                        Some(link),
                        &universe.projects,
                        None,
                    );
                    accumulate_row(
                        &mut total_input,
                        row,
                        w,
                        rebate_rate,
                        svc_rate,
                        fund_ratio_override,
                    );
                    accumulate_row(
                        buckets.entry(key).or_default(),
                        row,
                        w,
                        rebate_rate,
                        svc_rate,
                        fund_ratio_override,
                    );
                }
            }
            continue;
        }

        // Standard grouping (Date / Platform / Account / Entity / Content).
        let proj_weight = if project_filter.is_empty() {
            1.0
        } else {
            matching_proj_links
                .iter()
                .filter(|l| project_filter.contains(&l.project_id))
                .map(|l| l.allocation_weight)
                .sum::<f64>()
                .min(1.0)
        };
        let w = person_weight * proj_weight;
        let key = build_group_key(
            row,
            &group_dims,
            acct,
            matching_proj_links.first().copied(),
            &universe.projects,
            None,
        );
        accumulate_row(
            &mut total_input,
            row,
            w,
            rebate_rate,
            svc_rate,
            fund_ratio_override,
        );
        accumulate_row(
            buckets.entry(key).or_default(),
            row,
            w,
            rebate_rate,
            svc_rate,
            fund_ratio_override,
        );
    }

    let summary =
        CaliberEvaluator::evaluate("TOTAL", &total_input, &universe.custom_calibers)?;
    let mut groups = Vec::with_capacity(buckets.len());
    for (label, input) in buckets {
        groups.push(CaliberEvaluator::evaluate(
            label,
            &input,
            &universe.custom_calibers,
        )?);
    }
    Ok((summary, groups))
}

fn accumulate_row(
    target: &mut CaliberInput,
    row: &UnifiedReportRow,
    weight: f64,
    rebate_rate: f64,
    service_fee_rate: f64,
    fund_cash_ratio_override: Option<f64>,
) {
    let gross = row.gross_cost_fen.scale(weight);
    let (cash, grant) = match fund_cash_ratio_override {
        Some(ratio) if gross > Fen::ZERO => {
            let c = gross.scale(ratio.clamp(0.0, 1.0));
            (c, gross - c)
        }
        _ => (row.cash_cost_fen.scale(weight), row.grant_cost_fen.scale(weight)),
    };
    target.gross_cost_fen += gross;
    target.cash_cost_fen += cash;
    target.grant_cost_fen += grant;
    target.kol_order_fee_fen += row.kol_order_fee_fen.scale(weight);
    target.platform_service_fee_fen += row.platform_service_fee_fen.scale(weight);
    target.gmv_fen += row.gmv_fen.scale(weight);
    target.impressions = target
        .impressions
        .saturating_add((row.impressions as f64 * weight).round() as u64);
    target.clicks = target
        .clicks
        .saturating_add((row.clicks as f64 * weight).round() as u64);
    target.interactions = target
        .interactions
        .saturating_add((row.interactions as f64 * weight).round() as u64);
    target.conversions = target
        .conversions
        .saturating_add((row.conversions as f64 * weight).round() as u64);
    target.leads = target
        .leads
        .saturating_add((row.leads as f64 * weight).round() as u64);
    target.orders = target
        .orders
        .saturating_add((row.orders as f64 * weight).round() as u64);
    target.store_verifies = target
        .store_verifies
        .saturating_add((row.store_verifies as f64 * weight).round() as u64);
    target.weighted_rebate_fen += gross.scale(rebate_rate);
    target.weighted_agency_fee_fen += gross.scale(service_fee_rate);
}

fn build_group_key(
    row: &UnifiedReportRow,
    dims: &[QueryDimension],
    account: Option<&AdAccount>,
    project_link: Option<&ProjectAccountLink>,
    projects: &HashMap<String, Project>,
    person: Option<(&str, &str)>,
) -> String {
    dims.iter()
        .map(|dim| match dim {
            QueryDimension::Date => row.stat_date.to_string(),
            QueryDimension::Platform => row.platform.as_str().to_string(),
            QueryDimension::Account => account
                .map(|a| format!("{} ({})", a.account_name, a.id))
                .unwrap_or_else(|| row.ad_account_id.clone()),
            QueryDimension::Project => project_link
                .and_then(|l| projects.get(&l.project_id))
                .map(|p| format!("{} ({})", p.name, p.code))
                .unwrap_or_else(|| "unassigned_project".into()),
            QueryDimension::ProjectType => project_link
                .and_then(|l| projects.get(&l.project_id))
                .map(|p| project_type_label(&p.project_type).to_string())
                .unwrap_or_else(|| "unclassified".into()),
            QueryDimension::Person => person
                .map(|(id, name)| format!("{name} ({id})"))
                .unwrap_or_else(|| "unassigned".into()),
            QueryDimension::Entity => format!("{} ({})", row.entity_name, row.entity_id),
            QueryDimension::Content => row
                .content_id
                .clone()
                .unwrap_or_else(|| "no_content_id".into()),
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn project_type_label(pt: &ProjectType) -> &str {
    match pt {
        ProjectType::PerformanceAcquisition => "performance_acquisition",
        ProjectType::EcommerceGmv => "ecommerce_gmv",
        ProjectType::SeedingContent => "seeding_content",
        ProjectType::LocalLife => "local_life",
        ProjectType::BrandAwareness => "brand_awareness",
        ProjectType::LiveStreaming => "live_streaming",
        ProjectType::Custom(s) => s.as_str(),
    }
}

/// Computes per-creative performance attribution, per-creator workload ROI, and
/// cross-platform seeding + boost attribution (`note_id`/`bvid`).
pub fn compute_creative_attribution(
    assets: &[CreativeAsset],
    links: &[CreativePlatformLink],
    contributions: &[CreativeProductionContribution],
    people: &HashMap<String, Person>,
    reports: &[UnifiedReportRow],
    project_filter: Option<&str>,
    format_filter: Option<&CreativeFormat>,
    hook_filter: Option<&CreativeHookType>,
) -> (
    Vec<CreativeAttributionRow>,
    Vec<CreatorWorkloadSummary>,
    Vec<SeedingBoostAttributionRow>,
) {
    let mut asset_links: HashMap<&str, Vec<&CreativePlatformLink>> = HashMap::new();
    for link in links {
        asset_links
            .entry(link.asset_id.as_str())
            .or_default()
            .push(link);
    }

    let mut creative_rows = Vec::new();
    let mut asset_perf_map: HashMap<String, (Fen, Fen, u64)> = HashMap::new();

    for asset in assets {
        if let Some(pid) = project_filter {
            if asset.project_id != pid {
                continue;
            }
        }
        if let Some(fmt) = format_filter {
            if &asset.taxonomy.format != fmt {
                continue;
            }
        }
        if let Some(hk) = hook_filter {
            if &asset.taxonomy.hook_type != hk {
                continue;
            }
        }

        let my_links = asset_links
            .get(asset.id.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[]);

        let mut media_cost = Fen::ZERO;
        let mut kol_cost = Fen::ZERO;
        let mut impressions = 0u64;
        let mut clicks = 0u64;
        let mut interactions = 0u64;
        let mut conversions = 0u64;
        let mut gmv = Fen::ZERO;

        // Deduplicate matched rows per asset if both entity_id and content_id point to the same row.
        for (idx, row) in reports.iter().enumerate() {
            let _ = idx;
            let matches = my_links.iter().any(|l| {
                let direct = l.platform == row.platform
                    && l.ad_account_id == row.ad_account_id
                    && l.platform_creative_id == row.entity_id;
                let by_content = l
                    .platform_content_id
                    .as_ref()
                    .zip(row.content_id.as_ref())
                    .is_some_and(|(a, b)| a == b);
                direct || by_content
            });
            if !matches {
                continue;
            }
            // Prefer creative/content_order levels to avoid double counting with account/campaign rollups.
            if !matches!(
                row.level,
                ReportLevel::Creative | ReportLevel::ContentOrder | ReportLevel::AdGroup
            ) {
                continue;
            }
            media_cost += row.gross_cost_fen;
            kol_cost += row.kol_order_fee_fen + row.platform_service_fee_fen;
            impressions = impressions.saturating_add(row.impressions);
            clicks = clicks.saturating_add(row.clicks);
            interactions = interactions.saturating_add(row.interactions);
            conversions = conversions.saturating_add(row.conversions);
            gmv += row.gmv_fen;
        }

        let total_cost_yuan = (media_cost + kol_cost).to_yuan_f64();
        let gmv_yuan = gmv.to_yuan_f64();
        let hours = asset.production_hours.max(0.0);
        let safe_div = |a: f64, b: f64| if b.abs() < 1e-9 { 0.0 } else { a / b };
        let r2 = |v: f64| (v * 100.0).round() / 100.0;
        let r4 = |v: f64| (v * 10000.0).round() / 10000.0;

        asset_perf_map.insert(asset.id.clone(), (media_cost + kol_cost, gmv, conversions));

        creative_rows.push(CreativeAttributionRow {
            asset_id: asset.id.clone(),
            asset_code: asset.asset_code.clone(),
            title: asset.title.clone(),
            project_id: asset.project_id.clone(),
            format: asset.taxonomy.format.clone(),
            hook_type: asset.taxonomy.hook_type.clone(),
            scene_tag: asset.taxonomy.scene_tag.clone(),
            production_hours: hours,
            production_cost_fen: asset.production_cost_fen,
            media_gross_cost_fen: media_cost,
            kol_seeding_cost_fen: kol_cost,
            impressions,
            clicks,
            interactions,
            conversions,
            gmv_fen: gmv,
            ctr: r4(safe_div(clicks as f64, impressions as f64)),
            roi: r4(safe_div(gmv_yuan, total_cost_yuan)),
            gmv_per_hour_yuan: r2(safe_div(gmv_yuan, hours)),
            spend_carried_per_hour_yuan: r2(safe_div(total_cost_yuan, hours)),
            production_cost_ratio: r4(safe_div(
                asset.production_cost_fen.to_yuan_f64(),
                total_cost_yuan,
            )),
        });
    }

    creative_rows.sort_by(|a, b| b.gmv_fen.cmp(&a.gmv_fen));

    // Rollup creator workload & attributed ROI using `credit_weight`.
    struct CreatorAcc {
        assets: HashSet<String>,
        hours: f64,
        piece_cost: Fen,
        spend: Fen,
        gmv: Fen,
        conversions: f64,
    }
    let mut creator_map: HashMap<String, CreatorAcc> = HashMap::new();
    for contrib in contributions {
        let Some((asset_spend, asset_gmv, asset_conv)) = asset_perf_map.get(&contrib.asset_id)
        else {
            continue;
        };
        let entry = creator_map
            .entry(contrib.person_id.clone())
            .or_insert_with(|| CreatorAcc {
                assets: HashSet::new(),
                hours: 0.0,
                piece_cost: Fen::ZERO,
                spend: Fen::ZERO,
                gmv: Fen::ZERO,
                conversions: 0.0,
            });
        entry.assets.insert(contrib.asset_id.clone());
        entry.hours += contrib.work_hours;
        entry.piece_cost += contrib.piece_cost_fen;
        entry.spend += asset_spend.scale(contrib.credit_weight);
        entry.gmv += asset_gmv.scale(contrib.credit_weight);
        entry.conversions += *asset_conv as f64 * contrib.credit_weight;
    }

    let mut creator_summaries: Vec<CreatorWorkloadSummary> = creator_map
        .into_iter()
        .map(|(person_id, acc)| {
            let p = people.get(&person_id);
            let spend_yuan = acc.spend.to_yuan_f64();
            let gmv_yuan = acc.gmv.to_yuan_f64();
            let safe_div = |a: f64, b: f64| if b.abs() < 1e-9 { 0.0 } else { a / b };
            let r2 = |v: f64| (v * 100.0).round() / 100.0;
            let r4 = |v: f64| (v * 10000.0).round() / 10000.0;
            CreatorWorkloadSummary {
                person_id: person_id.clone(),
                person_name: p.map(|x| x.name.clone()).unwrap_or(person_id),
                department: p.map(|x| x.department.clone()).unwrap_or_default(),
                assets_touched: acc.assets.len(),
                total_work_hours: r2(acc.hours),
                weighted_piece_cost_fen: acc.piece_cost,
                attributed_media_spend_fen: acc.spend,
                attributed_gmv_fen: acc.gmv,
                attributed_conversions: r2(acc.conversions),
                gmv_per_work_hour_yuan: r2(safe_div(gmv_yuan, acc.hours)),
                spend_per_work_hour_yuan: r2(safe_div(spend_yuan, acc.hours)),
                avg_attributed_roi: r4(safe_div(gmv_yuan, spend_yuan)),
            }
        })
        .collect();
    creator_summaries.sort_by(|a, b| b.attributed_gmv_fen.cmp(&a.attributed_gmv_fen));

    // Cross-platform Seeding + Boost rollup keyed by `content_id` (note_id / bvid).
    let mut content_groups: BTreeMap<String, Vec<&UnifiedReportRow>> = BTreeMap::new();
    for row in reports {
        if !matches!(
            row.level,
            ReportLevel::Creative | ReportLevel::ContentOrder
        ) {
            continue;
        }
        if let Some(cid) = &row.content_id {
            if !cid.is_empty() {
                content_groups.entry(cid.clone()).or_default().push(row);
            }
        }
    }

    let mut seeding_boost = Vec::new();
    for (content_id, grp) in content_groups {
        let mut seeding_platform = None;
        let mut boost_platform = None;
        let mut kol_order = Fen::ZERO;
        let mut kol_svc = Fen::ZERO;
        let mut boost_cost = Fen::ZERO;
        let mut imps = 0u64;
        let mut clicks = 0u64;
        let mut inter = 0u64;
        let mut conv = 0u64;
        let mut gmv = Fen::ZERO;

        for r in grp {
            if matches!(
                r.platform,
                PlatformId::XhsPugongying | PlatformId::BilibiliHuahuo
            ) {
                seeding_platform = Some(r.platform);
            } else {
                boost_platform = Some(r.platform);
            }
            kol_order += r.kol_order_fee_fen;
            kol_svc += r.platform_service_fee_fen;
            boost_cost += r.gross_cost_fen;
            imps = imps.saturating_add(r.impressions);
            clicks = clicks.saturating_add(r.clicks);
            inter = inter.saturating_add(r.interactions);
            conv = conv.saturating_add(r.conversions);
            gmv += r.gmv_fen;
        }

        if let Some(seed_p) = seeding_platform {
            let total_cost = kol_order + kol_svc + boost_cost;
            let total_yuan = total_cost.to_yuan_f64();
            let gmv_yuan = gmv.to_yuan_f64();
            let cpe = if inter == 0 {
                0.0
            } else {
                ((total_yuan / inter as f64) * 100.0).round() / 100.0
            };
            let roi = if total_yuan <= 0.0 {
                0.0
            } else {
                ((gmv_yuan / total_yuan) * 10000.0).round() / 10000.0
            };
            seeding_boost.push(SeedingBoostAttributionRow {
                content_id,
                seeding_platform: seed_p,
                boost_platform,
                kol_order_fee_fen: kol_order,
                kol_service_fee_fen: kol_svc,
                paid_boost_cost_fen: boost_cost,
                total_full_funnel_cost_fen: total_cost,
                total_impressions: imps,
                total_clicks: clicks,
                total_interactions: inter,
                total_conversions: conv,
                total_gmv_fen: gmv,
                full_funnel_cpe_yuan: cpe,
                full_funnel_roi: roi,
            });
        }
    }

    (creative_rows, creator_summaries, seeding_boost)
}

fn select_canonical_level_rows<'a>(
    rows: &'a [UnifiedReportRow],
    group_dims: &[QueryDimension],
) -> Vec<&'a UnifiedReportRow> {
    let mut available: HashMap<(&str, NaiveDate), (PlatformId, HashSet<ReportLevel>)> =
        HashMap::new();
    for row in rows {
        available
            .entry((row.ad_account_id.as_str(), row.stat_date))
            .or_insert_with(|| (row.platform, HashSet::new()))
            .1
            .insert(row.level);
    }

    let fine_grained = group_dims
        .iter()
        .any(|d| matches!(d, QueryDimension::Entity | QueryDimension::Content));

    let mut winners: HashMap<(&str, NaiveDate), ReportLevel> =
        HashMap::with_capacity(available.len());
    for (key, (platform, levels)) in available {
        let priority: &[ReportLevel] = if fine_grained {
            &[
                ReportLevel::ContentOrder,
                ReportLevel::Creative,
                ReportLevel::AdGroup,
                ReportLevel::Campaign,
                ReportLevel::Account,
            ]
        } else if matches!(
            platform,
            PlatformId::XhsPugongying | PlatformId::BilibiliHuahuo
        ) {
            &[
                ReportLevel::ContentOrder,
                ReportLevel::Creative,
                ReportLevel::Account,
                ReportLevel::Campaign,
                ReportLevel::AdGroup,
            ]
        } else {
            &[
                ReportLevel::Account,
                ReportLevel::Campaign,
                ReportLevel::AdGroup,
                ReportLevel::Creative,
                ReportLevel::ContentOrder,
            ]
        };

        if let Some(&win) = priority.iter().find(|lvl| levels.contains(lvl)) {
            winners.insert(key, win);
        }
    }

    rows.iter()
        .filter(|r| winners.get(&(r.ad_account_id.as_str(), r.stat_date)) == Some(&r.level))
        .collect()
}

fn dedup_granularity_and_timeline<'a>(
    rows: &[&'a UnifiedReportRow],
) -> Vec<&'a UnifiedReportRow> {
    let mut flags: HashMap<(&str, NaiveDate, ReportLevel), (bool, bool)> = HashMap::new();
    for r in rows {
        let entry = flags
            .entry((r.ad_account_id.as_str(), r.stat_date, r.level))
            .or_insert((false, false));
        if r.granularity == ReportGranularity::Daily {
            entry.0 = true;
        }
        if r.time_line == TimeLine::ReportingTime {
            entry.1 = true;
        }
    }

    rows.iter()
        .copied()
        .filter(|r| {
            let Some(&(has_daily, has_reporting)) =
                flags.get(&(r.ad_account_id.as_str(), r.stat_date, r.level))
            else {
                return true;
            };
            let ok_gran = !has_daily || r.granularity == ReportGranularity::Daily;
            let ok_tl = !has_reporting || r.time_line == TimeLine::ReportingTime;
            ok_gran && ok_tl
        })
        .collect()
}

pub fn parse_date(s: &str) -> Result<NaiveDate, StorageError> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| StorageError::DataIntegrity(format!("invalid date '{s}': {e}")))
}
