//! Meta Marketing API v21.0 (Facebook / Instagram Ads) overseas connector plugin.
//!
//! Official endpoints (`https://graph.facebook.com/v21.0`, Header: `Authorization: Bearer`):
//! - `GET /act_{ad_account_id}/insights` (`spend`, `impressions`, `clicks`, `actions`, `action_values`, `account_currency`)
//! - `GET /me/adaccounts`
//! - Opt-in creation: `POST /act_{id}/campaigns`, `POST /act_{id}/adsets`, `POST /act_{id}/ads` (`daily_budget` in cents)

use std::sync::Arc;

use adlink_domain::{
    AccountType, AdAccount, AdCreationEntityKind, AdCreationRequest, AdCreationResult, AdEntity,
    AdMutationAction, AdMutationRequest, AdMutationResult, AuthLoginStrategy, DailyFundStatement,
    DiscoveredPlatformAccount, Fen, PlatformId, ReportGranularity, ReportLevel, TimeLine,
    UnifiedReportRow,
};
use adlink_engine::{
    AdPlatformConnector, ConnectorError, ConnectorResult, PlatformOnboardGuide, ReportFetchPage,
    ReportFetchRequest,
};
use adlink_kernel::{KernelResult, Plugin, PluginCategory, PluginContext, PluginManifest};
use async_trait::async_trait;
use chrono::NaiveDate;
use reqwest::Client;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::http_helpers::{
    base_fen_to_currency_cents, build_http_client, discover_via_official_mcp_bridge,
    extract_raw_map, json_f64, json_opt_str, json_str, json_u64, map_http_status, merge_json_extra,
    mock_discovered_accounts, platform_creation_notes, resolve_account_fx,
    stamp_currency_raw_fields, ConnectorMode,
};
use crate::mock_data::mock_entities_for_account;
use crate::mock_generator::synthesize_chunk_metrics;
use crate::onboarding::build_onboard_guide;

pub struct MetaAdsConnector {
    mode: ConnectorMode,
    client: Client,
}

impl MetaAdsConnector {
    pub fn new(mode: ConnectorMode) -> Self {
        Self {
            mode,
            client: build_http_client(),
        }
    }

    fn base_url(&self) -> &str {
        if let ConnectorMode::Live(creds) = &self.mode {
            if let Some(u) = &creds.base_url_override {
                return u.as_str();
            }
        }
        "https://graph.facebook.com/v21.0"
    }

    fn act_path_id(raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.starts_with("act_") {
            trimmed.to_string()
        } else {
            format!("act_{trimmed}")
        }
    }

    fn find_action_u64(arr: Option<&Vec<Value>>, action_types: &[&str]) -> u64 {
        let Some(items) = arr else {
            return 0;
        };
        for target in action_types {
            for item in items {
                if item.get("action_type").and_then(Value::as_str) == Some(*target) {
                    return json_u64(item, "value");
                }
            }
        }
        0
    }

    fn find_action_value_f64(arr: Option<&Vec<Value>>, action_types: &[&str]) -> f64 {
        let Some(items) = arr else {
            return 0.0;
        };
        for target in action_types {
            for item in items {
                if item.get("action_type").and_then(Value::as_str) == Some(*target) {
                    return json_f64(item, "value");
                }
            }
        }
        0.0
    }

    /// Parses official Meta Graph API v21.0 `/insights` JSON payload.
    pub fn parse_official_reports_response(
        payload: &Value,
        account: &AdAccount,
        granularity: ReportGranularity,
        level: ReportLevel,
        time_line: TimeLine,
    ) -> ConnectorResult<ReportFetchPage> {
        if let Some(err) = payload.get("error") {
            let code = err.get("code").and_then(Value::as_i64).unwrap_or(-1);
            let msg = json_str(err, "message");
            if code == 17 || code == 4 || code == 613 || code == 80004 {
                return Err(ConnectorError::RateLimited {
                    platform: PlatformId::MetaAds,
                    retry_after_ms: Some(1500),
                    message: format!("Meta error {code}: {msg}"),
                });
            }
            if code == 190 || code == 102 {
                return Err(ConnectorError::AuthError {
                    platform: PlatformId::MetaAds,
                    message: format!("Meta OAuth error {code}: {msg}"),
                });
            }
            return Err(ConnectorError::InvalidRequest {
                platform: PlatformId::MetaAds,
                message: format!("Meta error {code}: {msg}"),
            });
        }

        let list = payload
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let mut rows = Vec::with_capacity(list.len());
        for item in list {
            let date_str = json_str(&item, "date_start");
            let stat_date = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d").map_err(|e| {
                ConnectorError::ParseError {
                    platform: PlatformId::MetaAds,
                    message: format!("invalid date_start '{date_str}': {e}"),
                }
            })?;
            let stat_hour = if granularity == ReportGranularity::Hourly {
                item.get("hourly_stats_aggregated_by_advertiser_time_zone")
                    .and_then(Value::as_str)
                    .and_then(|s| s.split(':').next())
                    .and_then(|h| h.parse::<i32>().ok())
                    .unwrap_or(-1)
            } else {
                -1
            };

            let (currency, fx_rate) = resolve_account_fx(
                account,
                item.get("account_currency").and_then(Value::as_str),
            );
            let spend_major = json_f64(&item, "spend");
            let gross_fen = Fen::from_yuan_f64(spend_major * fx_rate);
            let cash_fen = gross_fen.scale(account.default_cash_ratio);

            let actions = item.get("actions").and_then(Value::as_array);
            let action_values = item.get("action_values").and_then(Value::as_array);

            // Prioritize `omni_purchase` over `purchase` to prevent cross-channel double counting.
            let orders = Self::find_action_u64(
                actions,
                &[
                    "omni_purchase",
                    "purchase",
                    "offsite_conversion.fb_pixel_purchase",
                ],
            );
            let gmv_major = Self::find_action_value_f64(
                action_values,
                &[
                    "omni_purchase",
                    "purchase",
                    "offsite_conversion.fb_pixel_purchase",
                ],
            );
            let gmv_fen = Fen::from_yuan_f64(gmv_major * fx_rate);
            let leads = Self::find_action_u64(
                actions,
                &["lead", "onsite_conversion.lead_grouped", "offsite_conversion.fb_pixel_lead"],
            );
            let interactions = Self::find_action_u64(actions, &["post_engagement"]);
            let video_plays = Self::find_action_u64(actions, &["video_view"]);

            let entity_id = match level {
                ReportLevel::Account => json_opt_str(&item, "account_id")
                    .unwrap_or_else(|| account.platform_account_id.clone()),
                ReportLevel::Campaign => json_str(&item, "campaign_id"),
                ReportLevel::AdGroup => json_str(&item, "adset_id"),
                ReportLevel::Creative | ReportLevel::ContentOrder => json_str(&item, "ad_id"),
            };
            let entity_name = {
                let ad = json_str(&item, "ad_name");
                if !ad.is_empty() {
                    ad
                } else {
                    let adset = json_str(&item, "adset_name");
                    if !adset.is_empty() {
                        adset
                    } else {
                        let camp = json_str(&item, "campaign_name");
                        if camp.is_empty() {
                            account.account_name.clone()
                        } else {
                            camp
                        }
                    }
                }
            };

            let mut raw = extract_raw_map(&item);
            stamp_currency_raw_fields(&mut raw, &currency, fx_rate, spend_major);

            rows.push(UnifiedReportRow {
                platform: PlatformId::MetaAds,
                ad_account_id: account.id.clone(),
                platform_account_id: account.platform_account_id.clone(),
                stat_date,
                stat_hour,
                granularity,
                level,
                entity_id: if entity_id.is_empty() {
                    account.platform_account_id.clone()
                } else {
                    entity_id
                },
                entity_name,
                parent_entity_id: json_opt_str(&item, "campaign_id")
                    .or_else(|| json_opt_str(&item, "adset_id")),
                content_id: json_opt_str(&item, "ad_id"),
                time_line,
                gross_cost_fen: gross_fen,
                cash_cost_fen: cash_fen,
                grant_cost_fen: gross_fen - cash_fen,
                impressions: json_u64(&item, "impressions"),
                clicks: json_u64(&item, "clicks").max(json_u64(&item, "inline_link_clicks")),
                conversions: orders.max(leads),
                leads,
                orders,
                gmv_fen,
                interactions,
                video_plays,
                raw_fields: raw,
                ..Default::default()
            });
        }

        let paging = payload.get("paging").unwrap_or(&Value::Null);
        let next_cursor = paging
            .get("cursors")
            .and_then(|c| c.get("after"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let has_more = paging.get("next").is_some();

        Ok(ReportFetchPage {
            rows,
            has_more,
            total_count: None,
            next_cursor,
        })
    }

    fn build_mock_payload(req: &ReportFetchRequest) -> Value {
        let (currency, fx_rate) = resolve_account_fx(&req.account, None);
        let (items, has_more, _) = synthesize_chunk_metrics(
            &req.account,
            req.start_date,
            req.end_date,
            req.granularity,
            req.level,
            req.page,
            req.page_size,
        );
        let data: Vec<Value> = items
            .into_iter()
            .map(|(dt, m)| {
                let spend_usd = ((m.gross_cost_fen.to_yuan_f64() / fx_rate) * 100.0).round() / 100.0;
                let gmv_usd = ((m.gmv_fen.to_yuan_f64() / fx_rate) * 100.0).round() / 100.0;
                let ds = dt.format("%Y-%m-%d").to_string();
                json!({
                    "date_start": ds,
                    "date_stop": ds,
                    "account_id": req.account.platform_account_id,
                    "account_currency": currency,
                    "campaign_id": m.parent_entity_id.clone().unwrap_or_else(|| "meta_camp_01".into()),
                    "campaign_name": m.entity_name,
                    "adset_id": m.entity_id,
                    "adset_name": m.entity_name,
                    "ad_id": m.entity_id,
                    "ad_name": m.entity_name,
                    "spend": format!("{spend_usd:.2}"),
                    "impressions": m.impressions.to_string(),
                    "clicks": m.clicks.to_string(),
                    "hourly_stats_aggregated_by_advertiser_time_zone": if m.stat_hour >= 0 { json!(format!("{:02}:00:00", m.stat_hour)) } else { Value::Null },
                    "actions": [
                        { "action_type": "omni_purchase", "value": m.orders.to_string() },
                        { "action_type": "lead", "value": m.leads.to_string() },
                        { "action_type": "post_engagement", "value": m.interactions.to_string() },
                        { "action_type": "video_view", "value": m.video_plays.to_string() }
                    ],
                    "action_values": [
                        { "action_type": "omni_purchase", "value": format!("{gmv_usd:.2}") }
                    ]
                })
            })
            .collect();
        let paging = if has_more {
            json!({
                "cursors": { "after": format!("page_{}", req.page + 1) },
                "next": "https://graph.facebook.com/v21.0/next"
            })
        } else {
            json!({})
        };
        json!({ "data": data, "paging": paging })
    }
}

#[async_trait]
impl AdPlatformConnector for MetaAdsConnector {
    fn platform_id(&self) -> PlatformId {
        PlatformId::MetaAds
    }

    fn mode_label(&self) -> &'static str {
        match self.mode {
            ConnectorMode::Mock => "mock",
            ConnectorMode::Live(_) => "live",
        }
    }

    fn onboard_guide(&self) -> PlatformOnboardGuide {
        build_onboard_guide(PlatformId::MetaAds, self.mode_label())
    }

    async fn discover_accounts(&self) -> ConnectorResult<Vec<DiscoveredPlatformAccount>> {
        match &self.mode {
            ConnectorMode::Mock => Ok(mock_discovered_accounts(
                PlatformId::MetaAds,
                AuthLoginStrategy::OfficialMcpBridge,
            )),
            ConnectorMode::Live(creds) => {
                if let Some(bridged) =
                    discover_via_official_mcp_bridge(&self.client, PlatformId::MetaAds, creds).await
                {
                    return Ok(bridged);
                }
                let url = format!("{}/me/adaccounts", self.base_url());
                let resp = self
                    .client
                    .get(&url)
                    .bearer_auth(&creds.access_token)
                    .query(&[("fields", "account_id,name,currency,account_status"), ("limit", "100")])
                    .send()
                    .await
                    .map_err(|e| ConnectorError::Transient {
                        platform: PlatformId::MetaAds,
                        message: e.to_string(),
                    })?;
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if !status.is_success() {
                    return Err(map_http_status(PlatformId::MetaAds, status, &text, None));
                }
                let val: Value =
                    serde_json::from_str(&text).map_err(|e| ConnectorError::ParseError {
                        platform: PlatformId::MetaAds,
                        message: e.to_string(),
                    })?;
                let list = val
                    .get("data")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                Ok(list
                    .into_iter()
                    .filter_map(|item| {
                        let pid = json_str(&item, "account_id");
                        if pid.is_empty() {
                            return None;
                        }
                        let name = {
                            let n = json_str(&item, "name");
                            if n.is_empty() {
                                format!("Meta act_{pid}")
                            } else {
                                n
                            }
                        };
                        Some(DiscoveredPlatformAccount {
                            platform: PlatformId::MetaAds,
                            platform_account_id: pid,
                            account_name: name,
                            account_type: AccountType::AgencySub,
                            status: "active".into(),
                            parent_agent_id: None,
                            auth_strategy: AuthLoginStrategy::DirectApiAccountDiscovery,
                            already_registered_ad_account_id: None,
                            current_person_ids: Vec::new(),
                            current_project_ids: Vec::new(),
                            raw_extra: item,
                        })
                    })
                    .collect())
            }
        }
    }

    async fn fetch_reports_page(&self, req: &ReportFetchRequest) -> ConnectorResult<ReportFetchPage> {
        let payload = match &self.mode {
            ConnectorMode::Mock => Self::build_mock_payload(req),
            ConnectorMode::Live(creds) => {
                let act = Self::act_path_id(&req.account.platform_account_id);
                let url = format!("{}/{}/insights", self.base_url(), act);
                let level_str = match req.level {
                    ReportLevel::Account => "account",
                    ReportLevel::Campaign => "campaign",
                    ReportLevel::AdGroup => "adset",
                    ReportLevel::Creative | ReportLevel::ContentOrder => "ad",
                };
                let time_range = json!({
                    "since": req.start_date.format("%Y-%m-%d").to_string(),
                    "until": req.end_date.format("%Y-%m-%d").to_string(),
                })
                .to_string();
                let fields = "date_start,date_stop,account_id,account_currency,campaign_id,campaign_name,adset_id,adset_name,ad_id,ad_name,spend,impressions,clicks,inline_link_clicks,actions,action_values";
                let mut query_params = vec![
                    ("level", level_str.to_string()),
                    ("time_increment", "1".into()),
                    ("time_range", time_range),
                    ("fields", fields.into()),
                    ("limit", req.page_size.to_string()),
                ];
                if req.granularity == ReportGranularity::Hourly {
                    query_params.push((
                        "breakdowns",
                        "hourly_stats_aggregated_by_advertiser_time_zone".into(),
                    ));
                }
                let resp = self
                    .client
                    .get(&url)
                    .bearer_auth(&creds.access_token)
                    .query(&query_params)
                    .send()
                    .await
                    .map_err(|e| ConnectorError::Transient {
                        platform: PlatformId::MetaAds,
                        message: e.to_string(),
                    })?;
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if !status.is_success() {
                    return Err(map_http_status(PlatformId::MetaAds, status, &text, None));
                }
                serde_json::from_str(&text).map_err(|e| ConnectorError::ParseError {
                    platform: PlatformId::MetaAds,
                    message: e.to_string(),
                })?
            }
        };
        Self::parse_official_reports_response(
            &payload,
            &req.account,
            req.granularity,
            req.level,
            req.time_line,
        )
    }

    async fn fetch_fund_statements(
        &self,
        account: &AdAccount,
        start_date: NaiveDate,
        end_date: NaiveDate,
    ) -> ConnectorResult<Vec<DailyFundStatement>> {
        let mut out = Vec::new();
        let mut cur = start_date;
        while cur <= end_date {
            let gross = Fen::from_yuan_f64(7200.0);
            let cash = gross.scale(account.default_cash_ratio);
            out.push(DailyFundStatement {
                platform: PlatformId::MetaAds,
                ad_account_id: account.id.clone(),
                platform_account_id: account.platform_account_id.clone(),
                stat_date: cur,
                cash_balance_fen: Fen(320_000_00),
                grant_balance_fen: Fen(10_000_00),
                cash_cost_fen: cash,
                grant_cost_fen: gross - cash,
                shared_wallet_cost_fen: Fen::ZERO,
                income_fen: Fen::ZERO,
                raw_fields: Default::default(),
            });
            cur = match cur.succ_opt() {
                Some(n) => n,
                None => break,
            };
        }
        Ok(out)
    }

    async fn list_ad_entities(
        &self,
        account: &AdAccount,
        level: ReportLevel,
    ) -> ConnectorResult<Vec<AdEntity>> {
        Ok(mock_entities_for_account(account)
            .into_iter()
            .filter(|e| e.level == level)
            .collect())
    }

    async fn mutate_ad_entities(
        &self,
        account: &AdAccount,
        request: &AdMutationRequest,
    ) -> ConnectorResult<AdMutationResult> {
        if request.dry_run || matches!(self.mode, ConnectorMode::Mock) {
            return Ok(AdMutationResult {
                dry_run: request.dry_run,
                accepted: true,
                platform: PlatformId::MetaAds,
                ad_account_id: account.id.clone(),
                affected_entity_ids: request.entity_ids.clone(),
                summary: format!("Meta Graph API v21.0 {:?} applied ({})", request.action, self.mode_label()),
                guardrail_notes: vec![],
                platform_response: json!({"success": true}),
            });
        }
        if let ConnectorMode::Live(creds) = &self.mode {
            let entity_id = request.entity_ids.first().cloned().unwrap_or_default();
            let (_currency, fx_rate) = resolve_account_fx(account, None);
            let mut body = json!({});
            match &request.action {
                AdMutationAction::Pause => body["status"] = json!("PAUSED"),
                AdMutationAction::Resume => body["status"] = json!("ACTIVE"),
                AdMutationAction::UpdateBudget { daily_budget_fen } => {
                    body["daily_budget"] = json!(base_fen_to_currency_cents(*daily_budget_fen, fx_rate));
                }
                AdMutationAction::UpdateBid { bid_fen } => {
                    body["bid_amount"] = json!(base_fen_to_currency_cents(*bid_fen, fx_rate));
                }
            }
            let url = format!("{}/{}", self.base_url(), entity_id);
            let resp = self
                .client
                .post(&url)
                .bearer_auth(&creds.access_token)
                .json(&body)
                .send()
                .await
                .map_err(|e| ConnectorError::Transient {
                    platform: PlatformId::MetaAds,
                    message: e.to_string(),
                })?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(map_http_status(PlatformId::MetaAds, status, &text, None));
            }
            return Ok(AdMutationResult {
                dry_run: false,
                accepted: true,
                platform: PlatformId::MetaAds,
                ad_account_id: account.id.clone(),
                affected_entity_ids: request.entity_ids.clone(),
                summary: "Meta Graph API live mutation submitted".into(),
                guardrail_notes: vec![],
                platform_response: serde_json::from_str(&text).unwrap_or(json!({"raw": text})),
            });
        }
        unreachable!()
    }

    async fn create_ad_entity(
        &self,
        account: &AdAccount,
        request: &AdCreationRequest,
    ) -> ConnectorResult<AdCreationResult> {
        let (currency, fx_rate) = resolve_account_fx(account, None);
        let act = Self::act_path_id(&account.platform_account_id);
        let status_str = if request.initial_status.eq_ignore_ascii_case("PAUSED") {
            "PAUSED"
        } else {
            "ACTIVE"
        };
        let (path, mut body) = match request.entity_kind {
            AdCreationEntityKind::Campaign => {
                let mut p = json!({
                    "name": request.name,
                    "objective": request.marketing_goal.clone().unwrap_or_else(|| "OUTCOME_SALES".into()),
                    "status": status_str,
                    "special_ad_categories": [],
                });
                if let Some(b_fen) = request.daily_budget_fen {
                    p["daily_budget"] = json!(base_fen_to_currency_cents(b_fen, fx_rate));
                }
                (format!("/{act}/campaigns"), p)
            }
            AdCreationEntityKind::AdGroup => {
                let camp_id = request.parent_entity_id.clone().ok_or_else(|| {
                    ConnectorError::InvalidRequest {
                        platform: PlatformId::MetaAds,
                        message: "Meta adset creation requires parent_entity_id (campaign_id)".into(),
                    }
                })?;
                let mut p = json!({
                    "name": request.name,
                    "campaign_id": camp_id,
                    "billing_event": "IMPRESSIONS",
                    "optimization_goal": "OFFSITE_CONVERSIONS",
                    "status": status_str,
                    "targeting": if request.targeting.is_null() { json!({"geo_locations": {"countries": ["US"]}}) } else { request.targeting.clone() },
                });
                if let Some(b_fen) = request.daily_budget_fen {
                    p["daily_budget"] = json!(base_fen_to_currency_cents(b_fen, fx_rate));
                }
                if let Some(bid_fen) = request.bid_fen {
                    p["bid_amount"] = json!(base_fen_to_currency_cents(bid_fen, fx_rate));
                }
                (format!("/{act}/adsets"), p)
            }
            AdCreationEntityKind::Creative => {
                let adset_id = request.parent_entity_id.clone().ok_or_else(|| {
                    ConnectorError::InvalidRequest {
                        platform: PlatformId::MetaAds,
                        message: "Meta ad creation requires parent_entity_id (adset_id)".into(),
                    }
                })?;
                let p = json!({
                    "name": request.name,
                    "adset_id": adset_id,
                    "status": status_str,
                    "creative": request.creative_assets,
                });
                (format!("/{act}/ads"), p)
            }
        };
        merge_json_extra(&mut body, &request.platform_extra);
        let note = format!(
            "Meta v21.0 creation endpoints require `daily_budget` & `bid_amount` in minor integer CENTS of account currency ({currency}, fx={fx_rate}) — distinct from major currency decimal strings in /insights."
        );
        let risk_warnings = platform_creation_notes(PlatformId::MetaAds, Some(&note));

        if request.dry_run || matches!(self.mode, ConnectorMode::Mock) {
            let created_id = if request.dry_run {
                None
            } else {
                Some(format!(
                    "meta_new_{}",
                    &Uuid::new_v4().simple().to_string()[..8]
                ))
            };
            return Ok(AdCreationResult {
                dry_run: request.dry_run,
                accepted: true,
                platform: PlatformId::MetaAds,
                ad_account_id: account.id.clone(),
                entity_kind: request.entity_kind,
                created_entity_id: created_id.clone(),
                initial_status: request.initial_status.clone(),
                summary: format!(
                    "Meta Graph v21.0 {:?} creation {} ({})",
                    request.entity_kind,
                    if request.dry_run { "previewed" } else { "created in mock" },
                    self.mode_label()
                ),
                risk_warnings,
                guardrail_notes: vec![format!("Target endpoint: POST {}{}", self.base_url(), path)],
                platform_payload_preview: body,
                platform_response: json!({ "id": created_id }),
            });
        }

        if let ConnectorMode::Live(creds) = &self.mode {
            let url = format!("{}{}", self.base_url(), path);
            let resp = self
                .client
                .post(&url)
                .bearer_auth(&creds.access_token)
                .json(&body)
                .send()
                .await
                .map_err(|e| ConnectorError::Transient {
                    platform: PlatformId::MetaAds,
                    message: e.to_string(),
                })?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(map_http_status(PlatformId::MetaAds, status, &text, None));
            }
            let val: Value = serde_json::from_str(&text).unwrap_or(json!({"raw": text}));
            let created_id = json_opt_str(&val, "id");
            return Ok(AdCreationResult {
                dry_run: false,
                accepted: true,
                platform: PlatformId::MetaAds,
                ad_account_id: account.id.clone(),
                entity_kind: request.entity_kind,
                created_entity_id: created_id,
                initial_status: request.initial_status.clone(),
                summary: format!("Meta Graph v21.0 live {:?} created", request.entity_kind),
                risk_warnings,
                guardrail_notes: vec![],
                platform_payload_preview: body,
                platform_response: val,
            });
        }
        unreachable!()
    }
}

pub struct MetaAdsPlugin {
    connector: Arc<MetaAdsConnector>,
}

impl MetaAdsPlugin {
    pub fn new(mode: ConnectorMode) -> Self {
        Self {
            connector: Arc::new(MetaAdsConnector::new(mode)),
        }
    }
}

#[async_trait]
impl Plugin for MetaAdsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "connector.meta_ads".into(),
            name: "Meta Ads Connector (Graph API v21.0)".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Official Marketing API v21.0 connector for Facebook & Instagram Ads".into(),
            category: PluginCategory::Connector,
            provides: vec!["connector:meta_ads".into()],
            requires: vec!["storage".into()],
            config_schema: json!({"type": "object"}),
        }
    }

    async fn mount(&self, ctx: &mut PluginContext) -> KernelResult<()> {
        let svc: Arc<dyn AdPlatformConnector> =
            Arc::clone(&self.connector) as Arc<dyn AdPlatformConnector>;
        ctx.provide_named_service(PlatformId::MetaAds.as_str(), svc);
        Ok(())
    }
}
