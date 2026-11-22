//! TikTok For Business (Marketing API v1.3) overseas connector plugin.
//!
//! Official endpoints (`https://business-api.tiktok.com/open_api/v1.3`, Header: `Access-Token`):
//! - `GET /report/integrated/get/` (`report_type=BASIC`, `data_level=AUCTION_*`)
//! - `GET /oauth2/advertiser/get/`
//! - `POST /campaign/status/update/`, `POST /adgroup/status/update/`
//! - Opt-in creation: `POST /campaign/create/`, `POST /adgroup/create/`, `POST /ad/create/`

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
    base_fen_to_currency_major, build_http_client, discover_via_official_mcp_bridge,
    extract_raw_map, json_f64, json_opt_str, json_str, json_u64, map_http_status, merge_json_extra,
    mock_discovered_accounts, platform_creation_notes, resolve_account_fx,
    stamp_currency_raw_fields, ConnectorMode,
};
use crate::mock_data::mock_entities_for_account;
use crate::mock_generator::synthesize_chunk_metrics;
use crate::onboarding::build_onboard_guide;

pub struct TikTokAdsConnector {
    mode: ConnectorMode,
    client: Client,
}

impl TikTokAdsConnector {
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
        "https://business-api.tiktok.com/open_api/v1.3"
    }

    /// Parses official TikTok Marketing API v1.3 `/report/integrated/get/` JSON response.
    pub fn parse_official_reports_response(
        payload: &Value,
        account: &AdAccount,
        granularity: ReportGranularity,
        level: ReportLevel,
        time_line: TimeLine,
    ) -> ConnectorResult<ReportFetchPage> {
        let code = payload.get("code").and_then(Value::as_i64).unwrap_or(0);
        if code != 0 {
            let msg = payload
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("tiktok api error");
            if code == 40100 || code == 40001 || msg.contains("rate") {
                return Err(ConnectorError::RateLimited {
                    platform: PlatformId::TikTokAds,
                    retry_after_ms: Some(1000),
                    message: format!("TikTok code {code}: {msg}"),
                });
            }
            if code == 40105 || code == 40002 || msg.contains("token") {
                return Err(ConnectorError::AuthError {
                    platform: PlatformId::TikTokAds,
                    message: format!("TikTok code {code}: {msg}"),
                });
            }
            return Err(ConnectorError::InvalidRequest {
                platform: PlatformId::TikTokAds,
                message: format!("TikTok code {code}: {msg}"),
            });
        }

        let data = payload.get("data").unwrap_or(payload);
        let list = data
            .get("list")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let mut rows = Vec::with_capacity(list.len());
        for item in list {
            let dims = item.get("dimensions").unwrap_or(&item);
            let mets = item.get("metrics").unwrap_or(&item);

            let date_raw = {
                let d = json_str(dims, "stat_time_day");
                if d.is_empty() {
                    json_str(dims, "stat_time_hour")
                } else {
                    d
                }
            };
            let date_only = date_raw.get(..10).unwrap_or(date_raw.as_str());
            let stat_date = NaiveDate::parse_from_str(date_only, "%Y-%m-%d").map_err(|e| {
                ConnectorError::ParseError {
                    platform: PlatformId::TikTokAds,
                    message: format!("invalid stat_time_day/hour '{date_raw}': {e}"),
                }
            })?;
            let stat_hour = if granularity == ReportGranularity::Hourly {
                dims.get("stat_time_hour")
                    .and_then(|v| {
                        v.as_i64().or_else(|| {
                            v.as_str().and_then(|s| {
                                s.split_whitespace()
                                    .nth(1)
                                    .unwrap_or(s)
                                    .split(':')
                                    .next()
                                    .and_then(|h| h.parse::<i64>().ok())
                            })
                        })
                    })
                    .map(|h| h as i32)
                    .unwrap_or(-1)
            } else {
                -1
            };

            let (currency, fx_rate) = resolve_account_fx(
                account,
                mets.get("currency")
                    .or_else(|| dims.get("currency"))
                    .and_then(Value::as_str),
            );
            let spend_major = json_f64(mets, "spend");
            let gross_fen = Fen::from_yuan_f64(spend_major * fx_rate);
            let cash_fen = gross_fen.scale(account.default_cash_ratio);

            let complete_payment = json_u64(mets, "complete_payment");
            let total_payment_value = json_f64(mets, "total_complete_payment_value");
            let value_per_payment = json_f64(mets, "value_per_complete_payment");
            let gmv_major = if total_payment_value > 0.0 {
                total_payment_value
            } else {
                complete_payment as f64 * value_per_payment
            };
            let gmv_fen = Fen::from_yuan_f64(gmv_major * fx_rate);

            let entity_id = match level {
                ReportLevel::Account => json_opt_str(dims, "advertiser_id")
                    .unwrap_or_else(|| account.platform_account_id.clone()),
                ReportLevel::Campaign => json_str(dims, "campaign_id"),
                ReportLevel::AdGroup => json_str(dims, "adgroup_id"),
                ReportLevel::Creative | ReportLevel::ContentOrder => json_str(dims, "ad_id"),
            };
            let entity_name = {
                let n = json_str(mets, "ad_name");
                if !n.is_empty() {
                    n
                } else {
                    let ag = json_str(mets, "adgroup_name");
                    if !ag.is_empty() {
                        ag
                    } else {
                        let c = json_str(mets, "campaign_name");
                        if c.is_empty() {
                            account.account_name.clone()
                        } else {
                            c
                        }
                    }
                }
            };

            let mut raw = extract_raw_map(dims);
            for (k, v) in extract_raw_map(mets) {
                raw.insert(k, v);
            }
            stamp_currency_raw_fields(&mut raw, &currency, fx_rate, spend_major);

            rows.push(UnifiedReportRow {
                platform: PlatformId::TikTokAds,
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
                parent_entity_id: json_opt_str(dims, "campaign_id")
                    .or_else(|| json_opt_str(dims, "adgroup_id")),
                content_id: json_opt_str(mets, "tiktok_item_id")
                    .or_else(|| json_opt_str(dims, "ad_id")),
                time_line,
                gross_cost_fen: gross_fen,
                cash_cost_fen: cash_fen,
                grant_cost_fen: gross_fen - cash_fen,
                impressions: json_u64(mets, "impressions"),
                clicks: json_u64(mets, "clicks"),
                conversions: json_u64(mets, "conversion").max(complete_payment),
                orders: complete_payment,
                gmv_fen,
                interactions: json_u64(mets, "likes")
                    + json_u64(mets, "comments")
                    + json_u64(mets, "shares"),
                video_plays: json_u64(mets, "video_play_actions"),
                effective_plays: json_u64(mets, "video_watched_6s"),
                new_follows: json_u64(mets, "follows"),
                raw_fields: raw,
                ..Default::default()
            });
        }

        let page_info = data.get("page_info").unwrap_or(&Value::Null);
        let cur = page_info.get("page").and_then(Value::as_u64).unwrap_or(1);
        let total_page = page_info
            .get("total_page")
            .and_then(Value::as_u64)
            .unwrap_or(1);

        Ok(ReportFetchPage {
            rows,
            has_more: cur < total_page,
            total_count: page_info.get("total_number").and_then(Value::as_u64),
            next_cursor: None,
        })
    }

    fn build_mock_payload(req: &ReportFetchRequest) -> Value {
        let (currency, fx_rate) = resolve_account_fx(&req.account, None);
        let (items, has_more, total) = synthesize_chunk_metrics(
            &req.account,
            req.start_date,
            req.end_date,
            req.granularity,
            req.level,
            req.page,
            req.page_size,
        );
        let list: Vec<Value> = items
            .into_iter()
            .map(|(dt, m)| {
                let spend_usd = ((m.gross_cost_fen.to_yuan_f64() / fx_rate) * 100.0).round() / 100.0;
                let gmv_usd = ((m.gmv_fen.to_yuan_f64() / fx_rate) * 100.0).round() / 100.0;
                let val_per_pay = if m.orders > 0 {
                    (gmv_usd / m.orders as f64 * 10000.0).round() / 10000.0
                } else {
                    0.0
                };
                let likes = (m.interactions as f64 * 0.65).round() as u64;
                let comments = (m.interactions as f64 * 0.20).round() as u64;
                let shares = m.interactions.saturating_sub(likes + comments);
                json!({
                    "dimensions": {
                        "stat_time_day": dt.format("%Y-%m-%d").to_string(),
                        "stat_time_hour": if m.stat_hour >= 0 { json!(format!("{} {:02}:00:00", dt, m.stat_hour)) } else { Value::Null },
                        "advertiser_id": req.account.platform_account_id,
                        "campaign_id": m.parent_entity_id.clone().unwrap_or_else(|| "tt_camp_01".into()),
                        "adgroup_id": m.entity_id,
                        "ad_id": m.entity_id,
                    },
                    "metrics": {
                        "currency": currency,
                        "campaign_name": m.entity_name,
                        "adgroup_name": m.entity_name,
                        "ad_name": m.entity_name,
                        "spend": format!("{spend_usd:.2}"),
                        "impressions": m.impressions,
                        "clicks": m.clicks,
                        "conversion": m.conversions,
                        "complete_payment": m.orders,
                        "value_per_complete_payment": format!("{val_per_pay:.4}"),
                        "total_complete_payment_value": format!("{gmv_usd:.2}"),
                        "likes": likes,
                        "comments": comments,
                        "shares": shares,
                        "follows": m.new_follows,
                        "video_play_actions": m.video_plays,
                        "video_watched_6s": m.effective_plays,
                        "tiktok_item_id": m.content_id,
                    }
                })
            })
            .collect();
        let total_page = if has_more { req.page + 1 } else { req.page };
        json!({
            "code": 0,
            "message": "OK",
            "data": {
                "list": list,
                "page_info": {
                    "page": req.page,
                    "page_size": req.page_size,
                    "total_number": total,
                    "total_page": total_page
                }
            }
        })
    }
}

#[async_trait]
impl AdPlatformConnector for TikTokAdsConnector {
    fn platform_id(&self) -> PlatformId {
        PlatformId::TikTokAds
    }

    fn mode_label(&self) -> &'static str {
        match self.mode {
            ConnectorMode::Mock => "mock",
            ConnectorMode::Live(_) => "live",
        }
    }

    fn onboard_guide(&self) -> PlatformOnboardGuide {
        build_onboard_guide(PlatformId::TikTokAds, self.mode_label())
    }

    async fn discover_accounts(&self) -> ConnectorResult<Vec<DiscoveredPlatformAccount>> {
        match &self.mode {
            ConnectorMode::Mock => Ok(mock_discovered_accounts(
                PlatformId::TikTokAds,
                AuthLoginStrategy::OfficialMcpBridge,
            )),
            ConnectorMode::Live(creds) => {
                if let Some(bridged) =
                    discover_via_official_mcp_bridge(&self.client, PlatformId::TikTokAds, creds)
                        .await
                {
                    return Ok(bridged);
                }
                let url = format!("{}/oauth2/advertiser/get/", self.base_url());
                let resp = self
                    .client
                    .get(&url)
                    .header("Access-Token", creds.access_token.as_str())
                    .query(&[
                        ("app_id", creds.app_id.as_str()),
                        ("secret", creds.app_secret.as_str()),
                    ])
                    .send()
                    .await
                    .map_err(|e| ConnectorError::Transient {
                        platform: PlatformId::TikTokAds,
                        message: e.to_string(),
                    })?;
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if !status.is_success() {
                    return Err(map_http_status(PlatformId::TikTokAds, status, &text, None));
                }
                let val: Value =
                    serde_json::from_str(&text).map_err(|e| ConnectorError::ParseError {
                        platform: PlatformId::TikTokAds,
                        message: e.to_string(),
                    })?;
                let list = val
                    .get("data")
                    .and_then(|d| d.get("list"))
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                Ok(list
                    .into_iter()
                    .filter_map(|item| {
                        let pid = json_str(&item, "advertiser_id");
                        if pid.is_empty() {
                            return None;
                        }
                        let name = {
                            let n = json_str(&item, "advertiser_name");
                            if n.is_empty() {
                                format!("TikTok Ads {pid}")
                            } else {
                                n
                            }
                        };
                        Some(DiscoveredPlatformAccount {
                            platform: PlatformId::TikTokAds,
                            platform_account_id: pid,
                            account_name: name,
                            account_type: AccountType::AgencySub,
                            status: "active".into(),
                            parent_agent_id: json_opt_str(&item, "owner_bc_id"),
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
                let data_level = match req.level {
                    ReportLevel::Account => "AUCTION_ADVERTISER",
                    ReportLevel::Campaign => "AUCTION_CAMPAIGN",
                    ReportLevel::AdGroup => "AUCTION_ADGROUP",
                    ReportLevel::Creative | ReportLevel::ContentOrder => "AUCTION_AD",
                };
                let time_dim = if req.granularity == ReportGranularity::Hourly {
                    "stat_time_hour"
                } else {
                    "stat_time_day"
                };
                let dims = match req.level {
                    ReportLevel::Account => json!([time_dim, "advertiser_id"]),
                    ReportLevel::Campaign => json!([time_dim, "campaign_id"]),
                    ReportLevel::AdGroup => json!([time_dim, "adgroup_id"]),
                    ReportLevel::Creative | ReportLevel::ContentOrder => json!([time_dim, "ad_id"]),
                };
                let metrics = json!([
                    "spend",
                    "impressions",
                    "clicks",
                    "conversion",
                    "complete_payment",
                    "value_per_complete_payment",
                    "total_complete_payment_value",
                    "likes",
                    "comments",
                    "shares",
                    "follows",
                    "video_play_actions",
                    "video_watched_6s",
                    "currency"
                ]);
                let url = format!("{}/report/integrated/get/", self.base_url());
                let resp = self
                    .client
                    .get(&url)
                    .header("Access-Token", creds.access_token.as_str())
                    .query(&[
                        ("advertiser_id", req.account.platform_account_id.as_str()),
                        ("report_type", "BASIC"),
                        ("service_type", "AUCTION"),
                        ("data_level", data_level),
                        ("dimensions", &dims.to_string()),
                        ("metrics", &metrics.to_string()),
                        ("start_date", &req.start_date.format("%Y-%m-%d").to_string()),
                        ("end_date", &req.end_date.format("%Y-%m-%d").to_string()),
                        ("page", &req.page.to_string()),
                        ("page_size", &req.page_size.to_string()),
                    ])
                    .send()
                    .await
                    .map_err(|e| ConnectorError::Transient {
                        platform: PlatformId::TikTokAds,
                        message: e.to_string(),
                    })?;
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if !status.is_success() {
                    return Err(map_http_status(PlatformId::TikTokAds, status, &text, None));
                }
                serde_json::from_str(&text).map_err(|e| ConnectorError::ParseError {
                    platform: PlatformId::TikTokAds,
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
            let gross = Fen::from_yuan_f64(5400.0);
            let cash = gross.scale(account.default_cash_ratio);
            out.push(DailyFundStatement {
                platform: PlatformId::TikTokAds,
                ad_account_id: account.id.clone(),
                platform_account_id: account.platform_account_id.clone(),
                stat_date: cur,
                cash_balance_fen: Fen(250_000_00),
                grant_balance_fen: Fen(12_000_00),
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
                platform: PlatformId::TikTokAds,
                ad_account_id: account.id.clone(),
                affected_entity_ids: request.entity_ids.clone(),
                summary: format!("TikTok Ads v1.3 {:?} applied ({})", request.action, self.mode_label()),
                guardrail_notes: vec![],
                platform_response: json!({"code": 0}),
            });
        }
        if let ConnectorMode::Live(creds) = &self.mode {
            let path = if request.entity_level == ReportLevel::AdGroup {
                "/adgroup/status/update/"
            } else {
                "/campaign/status/update/"
            };
            let op_status = match &request.action {
                AdMutationAction::Resume => "ENABLE",
                AdMutationAction::Pause => "DISABLE",
                _ => {
                    return Err(ConnectorError::InvalidRequest {
                        platform: PlatformId::TikTokAds,
                        message: "status/update supports Pause/Resume; budget updates use /campaign/update/ or /adgroup/update/".into(),
                    })
                }
            };
            let id_key = if request.entity_level == ReportLevel::AdGroup {
                "adgroup_ids"
            } else {
                "campaign_ids"
            };
            let mut body = json!({
                "advertiser_id": account.platform_account_id,
                "operation_status": op_status,
            });
            body[id_key] = json!(request.entity_ids);
            let url = format!("{}{}", self.base_url(), path);
            let resp = self
                .client
                .post(&url)
                .header("Access-Token", creds.access_token.as_str())
                .json(&body)
                .send()
                .await
                .map_err(|e| ConnectorError::Transient {
                    platform: PlatformId::TikTokAds,
                    message: e.to_string(),
                })?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(map_http_status(PlatformId::TikTokAds, status, &text, None));
            }
            return Ok(AdMutationResult {
                dry_run: false,
                accepted: true,
                platform: PlatformId::TikTokAds,
                ad_account_id: account.id.clone(),
                affected_entity_ids: request.entity_ids.clone(),
                summary: "TikTok Ads live status updated".into(),
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
        let op_status = if request.initial_status.eq_ignore_ascii_case("PAUSED") {
            "DISABLE"
        } else {
            "ENABLE"
        };
        let (path, mut body) = match request.entity_kind {
            AdCreationEntityKind::Campaign => {
                let mut p = json!({
                    "advertiser_id": account.platform_account_id,
                    "campaign_name": request.name,
                    "objective_type": request.marketing_goal.clone().unwrap_or_else(|| "WEB_CONVERSIONS".into()),
                    "operation_status": op_status,
                });
                if let Some(b_fen) = request.daily_budget_fen {
                    p["budget_mode"] = json!("BUDGET_MODE_DAY");
                    p["budget"] = json!(base_fen_to_currency_major(b_fen, fx_rate));
                }
                ("/campaign/create/", p)
            }
            AdCreationEntityKind::AdGroup => {
                let camp_id = request.parent_entity_id.clone().ok_or_else(|| {
                    ConnectorError::InvalidRequest {
                        platform: PlatformId::TikTokAds,
                        message: "TikTok adgroup creation requires parent_entity_id (campaign_id)".into(),
                    }
                })?;
                let mut p = json!({
                    "advertiser_id": account.platform_account_id,
                    "campaign_id": camp_id,
                    "adgroup_name": request.name,
                    "placement_type": "PLACEMENT_TYPE_AUTOMATIC",
                    "optimization_goal": "COMPLETE_PAYMENT",
                    "operation_status": op_status,
                });
                if let Some(b_fen) = request.daily_budget_fen {
                    p["budget_mode"] = json!("BUDGET_MODE_DAY");
                    p["budget"] = json!(base_fen_to_currency_major(b_fen, fx_rate));
                }
                if let Some(bid_fen) = request.bid_fen {
                    p["bid_price"] = json!(base_fen_to_currency_major(bid_fen, fx_rate));
                }
                ("/adgroup/create/", p)
            }
            AdCreationEntityKind::Creative => {
                let adgroup_id = request.parent_entity_id.clone().ok_or_else(|| {
                    ConnectorError::InvalidRequest {
                        platform: PlatformId::TikTokAds,
                        message: "TikTok ad creation requires parent_entity_id (adgroup_id)".into(),
                    }
                })?;
                let p = json!({
                    "advertiser_id": account.platform_account_id,
                    "adgroup_id": adgroup_id,
                    "creatives": [{
                        "ad_name": request.name,
                        "operation_status": op_status,
                        "landing_page_url": request.landing_url,
                        "creative_assets": request.creative_assets
                    }]
                });
                ("/ad/create/", p)
            }
        };
        merge_json_extra(&mut body, &request.platform_extra);
        let note = format!(
            "TikTok v1.3 creation uses major account currency ({currency}, fx={fx_rate}); base CNY budgets converted automatically."
        );
        let risk_warnings = platform_creation_notes(PlatformId::TikTokAds, Some(&note));

        if request.dry_run || matches!(self.mode, ConnectorMode::Mock) {
            let created_id = if request.dry_run {
                None
            } else {
                Some(format!(
                    "tt_new_{}",
                    &Uuid::new_v4().simple().to_string()[..8]
                ))
            };
            return Ok(AdCreationResult {
                dry_run: request.dry_run,
                accepted: true,
                platform: PlatformId::TikTokAds,
                ad_account_id: account.id.clone(),
                entity_kind: request.entity_kind,
                created_entity_id: created_id.clone(),
                initial_status: request.initial_status.clone(),
                summary: format!(
                    "TikTok Ads v1.3 {:?} creation {} ({})",
                    request.entity_kind,
                    if request.dry_run { "previewed" } else { "created in mock" },
                    self.mode_label()
                ),
                risk_warnings,
                guardrail_notes: vec![format!("Target endpoint: POST {}{}", self.base_url(), path)],
                platform_payload_preview: body,
                platform_response: json!({
                    "code": 0,
                    "mock": matches!(self.mode, ConnectorMode::Mock),
                    "data": { "id": created_id }
                }),
            });
        }

        if let ConnectorMode::Live(creds) = &self.mode {
            let url = format!("{}{}", self.base_url(), path);
            let resp = self
                .client
                .post(&url)
                .header("Access-Token", creds.access_token.as_str())
                .json(&body)
                .send()
                .await
                .map_err(|e| ConnectorError::Transient {
                    platform: PlatformId::TikTokAds,
                    message: e.to_string(),
                })?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(map_http_status(PlatformId::TikTokAds, status, &text, None));
            }
            let val: Value = serde_json::from_str(&text).unwrap_or(json!({"raw": text}));
            if val.get("code").and_then(Value::as_i64).unwrap_or(0) != 0 {
                return Err(ConnectorError::InvalidRequest {
                    platform: PlatformId::TikTokAds,
                    message: format!("TikTok create error: {val}"),
                });
            }
            let data = val.get("data").unwrap_or(&val);
            let created_id = json_opt_str(data, "campaign_id")
                .or_else(|| json_opt_str(data, "adgroup_id"))
                .or_else(|| json_opt_str(data, "ad_id"));
            return Ok(AdCreationResult {
                dry_run: false,
                accepted: true,
                platform: PlatformId::TikTokAds,
                ad_account_id: account.id.clone(),
                entity_kind: request.entity_kind,
                created_entity_id: created_id,
                initial_status: request.initial_status.clone(),
                summary: format!("TikTok Ads live {:?} created", request.entity_kind),
                risk_warnings,
                guardrail_notes: vec![],
                platform_payload_preview: body,
                platform_response: val,
            });
        }
        unreachable!()
    }
}

pub struct TikTokAdsPlugin {
    connector: Arc<TikTokAdsConnector>,
}

impl TikTokAdsPlugin {
    pub fn new(mode: ConnectorMode) -> Self {
        Self {
            connector: Arc::new(TikTokAdsConnector::new(mode)),
        }
    }
}

#[async_trait]
impl Plugin for TikTokAdsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "connector.tiktok_ads".into(),
            name: "TikTok For Business Connector (v1.3)".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Official Marketing API v1.3 connector for TikTok Ads".into(),
            category: PluginCategory::Connector,
            provides: vec!["connector:tiktok_ads".into()],
            requires: vec!["storage".into()],
            config_schema: json!({"type": "object"}),
        }
    }

    async fn mount(&self, ctx: &mut PluginContext) -> KernelResult<()> {
        let svc: Arc<dyn AdPlatformConnector> =
            Arc::clone(&self.connector) as Arc<dyn AdPlatformConnector>;
        ctx.provide_named_service(PlatformId::TikTokAds.as_str(), svc);
        Ok(())
    }
}
