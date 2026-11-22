//! Google Ads API v18 (GAQL `googleAds:searchStream` + `campaigns:mutate`) overseas connector.
//!
//! Official endpoints (`https://googleads.googleapis.com/v18`):
//! - Headers: `Authorization: Bearer {access_token}`, `developer-token`, optional `login-customer-id`
//! - `POST /customers/{customer_id}/googleAds:searchStream`
//! - `GET /customers:listAccessibleCustomers`
//! - Opt-in creation: `POST /customers/{id}/{campaigns,adGroups,adGroupAds}:mutate` (`cost_micros` / `amountMicros`)

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
    base_fen_to_currency_micros, build_http_client, discover_via_official_mcp_bridge,
    extract_raw_map, json_f64, json_i64, json_opt_str, json_str, json_u64, map_http_status,
    merge_json_extra, mock_discovered_accounts, platform_creation_notes, resolve_account_fx,
    stamp_currency_raw_fields, ConnectorMode,
};
use crate::mock_data::mock_entities_for_account;
use crate::mock_generator::synthesize_chunk_metrics;
use crate::onboarding::build_onboard_guide;

pub struct GoogleAdsConnector {
    mode: ConnectorMode,
    client: Client,
}

impl GoogleAdsConnector {
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
        "https://googleads.googleapis.com/v18"
    }

    fn clean_customer_id(raw: &str) -> String {
        raw.chars().filter(|c| c.is_ascii_digit()).collect()
    }

    /// Parses official Google Ads API v18 `googleAds:searchStream` JSON array (or batch object).
    pub fn parse_official_reports_response(
        payload: &Value,
        account: &AdAccount,
        granularity: ReportGranularity,
        level: ReportLevel,
        time_line: TimeLine,
    ) -> ConnectorResult<ReportFetchPage> {
        if let Some(err) = payload.get("error") {
            let status_code = err.get("code").and_then(Value::as_i64).unwrap_or(400);
            let msg = json_str(err, "message");
            if status_code == 429 || msg.contains("RESOURCE_EXHAUSTED") {
                return Err(ConnectorError::RateLimited {
                    platform: PlatformId::GoogleAds,
                    retry_after_ms: Some(1500),
                    message: format!("Google Ads {status_code}: {msg}"),
                });
            }
            if status_code == 401 || status_code == 403 {
                return Err(ConnectorError::AuthError {
                    platform: PlatformId::GoogleAds,
                    message: format!("Google Ads {status_code}: {msg}"),
                });
            }
            return Err(ConnectorError::InvalidRequest {
                platform: PlatformId::GoogleAds,
                message: format!("Google Ads {status_code}: {msg}"),
            });
        }

        // `googleAds:searchStream` returns an array of batches `[ { "results": [...] }, ... ]`.
        let mut all_results = Vec::new();
        if let Some(batches) = payload.as_array() {
            for b in batches {
                if let Some(arr) = b.get("results").and_then(Value::as_array) {
                    all_results.extend(arr.iter().cloned());
                }
            }
        } else if let Some(arr) = payload.get("results").and_then(Value::as_array) {
            all_results.extend(arr.iter().cloned());
        }

        let mut rows = Vec::with_capacity(all_results.len());
        for item in all_results {
            let segments = item.get("segments").unwrap_or(&Value::Null);
            let customer = item.get("customer").unwrap_or(&Value::Null);
            let campaign = item.get("campaign").unwrap_or(&Value::Null);
            let ad_group = item.get("adGroup").unwrap_or(&Value::Null);
            let ad_group_ad = item.get("adGroupAd").unwrap_or(&Value::Null);
            let ad_obj = ad_group_ad.get("ad").unwrap_or(&Value::Null);
            let metrics = item.get("metrics").unwrap_or(&Value::Null);

            let date_str = json_str(segments, "date");
            let stat_date = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d").map_err(|e| {
                ConnectorError::ParseError {
                    platform: PlatformId::GoogleAds,
                    message: format!("invalid segments.date '{date_str}': {e}"),
                }
            })?;
            let stat_hour = if granularity == ReportGranularity::Hourly {
                segments
                    .get("hour")
                    .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                    .map(|h| h as i32)
                    .unwrap_or(-1)
            } else {
                -1
            };

            let (currency, fx_rate) = resolve_account_fx(
                account,
                customer.get("currencyCode").and_then(Value::as_str),
            );
            // Official Google Ads API uses `costMicros` (1_000_000 micros = 1 currency unit).
            let cost_micros = json_i64(metrics, "costMicros").max(json_i64(metrics, "cost_micros"));
            let cost_major = cost_micros as f64 / 1_000_000.0;
            let gross_fen = Fen::from_yuan_f64(cost_major * fx_rate);
            let cash_fen = gross_fen.scale(account.default_cash_ratio);

            let conv_val_major = json_f64(metrics, "conversionsValue")
                .max(json_f64(metrics, "conversions_value"));
            let gmv_fen = Fen::from_yuan_f64(conv_val_major * fx_rate);

            let entity_id = match level {
                ReportLevel::Account => json_opt_str(customer, "id")
                    .unwrap_or_else(|| account.platform_account_id.clone()),
                ReportLevel::Campaign => json_str(campaign, "id"),
                ReportLevel::AdGroup => json_str(ad_group, "id"),
                ReportLevel::Creative | ReportLevel::ContentOrder => json_str(ad_obj, "id"),
            };
            let entity_name = {
                let ad_n = json_str(ad_obj, "name");
                if !ad_n.is_empty() {
                    ad_n
                } else {
                    let ag_n = json_str(ad_group, "name");
                    if !ag_n.is_empty() {
                        ag_n
                    } else {
                        let c_n = json_str(campaign, "name");
                        if c_n.is_empty() {
                            account.account_name.clone()
                        } else {
                            c_n
                        }
                    }
                }
            };

            let mut raw = extract_raw_map(&item);
            stamp_currency_raw_fields(&mut raw, &currency, fx_rate, cost_major);

            let conversions = json_f64(metrics, "conversions").round().max(0.0) as u64;
            rows.push(UnifiedReportRow {
                platform: PlatformId::GoogleAds,
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
                parent_entity_id: json_opt_str(campaign, "id")
                    .or_else(|| json_opt_str(ad_group, "id")),
                content_id: json_opt_str(ad_obj, "id"),
                time_line,
                gross_cost_fen: gross_fen,
                cash_cost_fen: cash_fen,
                grant_cost_fen: gross_fen - cash_fen,
                impressions: json_u64(metrics, "impressions"),
                clicks: json_u64(metrics, "clicks"),
                conversions,
                orders: conversions,
                gmv_fen,
                interactions: json_u64(metrics, "interactions"),
                video_plays: json_u64(metrics, "videoViews").max(json_u64(metrics, "video_views")),
                raw_fields: raw,
                ..Default::default()
            });
        }

        // searchStream returns full result stream without multi-page cursor.
        Ok(ReportFetchPage {
            rows,
            has_more: false,
            total_count: None,
            next_cursor: None,
        })
    }

    fn build_mock_payload(req: &ReportFetchRequest) -> Value {
        let (currency, fx_rate) = resolve_account_fx(&req.account, None);
        let (items, _, _) = synthesize_chunk_metrics(
            &req.account,
            req.start_date,
            req.end_date,
            req.granularity,
            req.level,
            1,
            req.page_size.max(500),
        );
        let results: Vec<Value> = items
            .into_iter()
            .map(|(dt, m)| {
                let cost_micros = base_fen_to_currency_micros(m.gross_cost_fen.as_fen(), fx_rate);
                let gmv_major = ((m.gmv_fen.to_yuan_f64() / fx_rate) * 100.0).round() / 100.0;
                json!({
                    "customer": {
                        "id": Self::clean_customer_id(&req.account.platform_account_id),
                        "currencyCode": currency,
                    },
                    "campaign": {
                        "id": m.parent_entity_id.clone().unwrap_or_else(|| "gads_camp_01".into()),
                        "name": m.entity_name,
                    },
                    "adGroup": {
                        "id": m.entity_id,
                        "name": m.entity_name,
                    },
                    "adGroupAd": {
                        "ad": {
                            "id": m.entity_id,
                            "name": m.entity_name,
                        }
                    },
                    "segments": {
                        "date": dt.format("%Y-%m-%d").to_string(),
                        "hour": if m.stat_hour >= 0 { json!(m.stat_hour) } else { Value::Null },
                    },
                    "metrics": {
                        "costMicros": cost_micros.to_string(),
                        "impressions": m.impressions.to_string(),
                        "clicks": m.clicks.to_string(),
                        "conversions": m.conversions as f64,
                        "conversionsValue": gmv_major,
                        "interactions": m.interactions.to_string(),
                        "videoViews": m.video_plays.to_string(),
                    }
                })
            })
            .collect();
        json!([{ "results": results }])
    }
}

#[async_trait]
impl AdPlatformConnector for GoogleAdsConnector {
    fn platform_id(&self) -> PlatformId {
        PlatformId::GoogleAds
    }

    fn mode_label(&self) -> &'static str {
        match self.mode {
            ConnectorMode::Mock => "mock",
            ConnectorMode::Live(_) => "live",
        }
    }

    fn onboard_guide(&self) -> PlatformOnboardGuide {
        build_onboard_guide(PlatformId::GoogleAds, self.mode_label())
    }

    async fn discover_accounts(&self) -> ConnectorResult<Vec<DiscoveredPlatformAccount>> {
        match &self.mode {
            ConnectorMode::Mock => Ok(mock_discovered_accounts(
                PlatformId::GoogleAds,
                AuthLoginStrategy::OfficialMcpBridge,
            )),
            ConnectorMode::Live(creds) => {
                if let Some(bridged) =
                    discover_via_official_mcp_bridge(&self.client, PlatformId::GoogleAds, creds)
                        .await
                {
                    return Ok(bridged);
                }
                let url = format!("{}/customers:listAccessibleCustomers", self.base_url());
                let mut builder = self
                    .client
                    .get(&url)
                    .bearer_auth(&creds.access_token)
                    .header("developer-token", creds.developer_token.as_str());
                if !creds.login_customer_id.trim().is_empty() {
                    builder = builder.header("login-customer-id", creds.login_customer_id.as_str());
                }
                let resp = builder.send().await.map_err(|e| ConnectorError::Transient {
                    platform: PlatformId::GoogleAds,
                    message: e.to_string(),
                })?;
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if !status.is_success() {
                    return Err(map_http_status(PlatformId::GoogleAds, status, &text, None));
                }
                let val: Value =
                    serde_json::from_str(&text).map_err(|e| ConnectorError::ParseError {
                        platform: PlatformId::GoogleAds,
                        message: e.to_string(),
                    })?;
                let names = val
                    .get("resourceNames")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                Ok(names
                    .into_iter()
                    .filter_map(|rn| {
                        let s = rn.as_str()?;
                        let cid = s.strip_prefix("customers/").unwrap_or(s).to_string();
                        Some(DiscoveredPlatformAccount {
                            platform: PlatformId::GoogleAds,
                            platform_account_id: cid.clone(),
                            account_name: format!("Google Ads Customer {cid}"),
                            account_type: AccountType::AgencySub,
                            status: "active".into(),
                            parent_agent_id: (!creds.login_customer_id.is_empty())
                                .then(|| creds.login_customer_id.clone()),
                            auth_strategy: AuthLoginStrategy::DirectApiAccountDiscovery,
                            already_registered_ad_account_id: None,
                            current_person_ids: Vec::new(),
                            current_project_ids: Vec::new(),
                            raw_extra: json!({ "resourceName": s }),
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
                let cid = Self::clean_customer_id(&req.account.platform_account_id);
                let url = format!("{}/customers/{cid}/googleAds:searchStream", self.base_url());
                let sd = req.start_date.format("%Y-%m-%d").to_string();
                let ed = req.end_date.format("%Y-%m-%d").to_string();
                let hour_sel = if req.granularity == ReportGranularity::Hourly {
                    ", segments.hour"
                } else {
                    ""
                };
                let gaql = match req.level {
                    ReportLevel::Account => format!(
                        "SELECT customer.id, customer.currency_code, segments.date{hour_sel}, metrics.cost_micros, metrics.impressions, metrics.clicks, metrics.conversions, metrics.conversions_value, metrics.interactions, metrics.video_views FROM customer WHERE segments.date BETWEEN '{sd}' AND '{ed}'"
                    ),
                    ReportLevel::Campaign => format!(
                        "SELECT customer.id, customer.currency_code, campaign.id, campaign.name, segments.date{hour_sel}, metrics.cost_micros, metrics.impressions, metrics.clicks, metrics.conversions, metrics.conversions_value, metrics.interactions, metrics.video_views FROM campaign WHERE segments.date BETWEEN '{sd}' AND '{ed}'"
                    ),
                    ReportLevel::AdGroup => format!(
                        "SELECT customer.id, customer.currency_code, campaign.id, ad_group.id, ad_group.name, segments.date{hour_sel}, metrics.cost_micros, metrics.impressions, metrics.clicks, metrics.conversions, metrics.conversions_value, metrics.interactions, metrics.video_views FROM ad_group WHERE segments.date BETWEEN '{sd}' AND '{ed}'"
                    ),
                    ReportLevel::Creative | ReportLevel::ContentOrder => format!(
                        "SELECT customer.id, customer.currency_code, campaign.id, ad_group.id, ad_group_ad.ad.id, ad_group_ad.ad.name, segments.date{hour_sel}, metrics.cost_micros, metrics.impressions, metrics.clicks, metrics.conversions, metrics.conversions_value, metrics.interactions, metrics.video_views FROM ad_group_ad WHERE segments.date BETWEEN '{sd}' AND '{ed}'"
                    ),
                };
                let mut builder = self
                    .client
                    .post(&url)
                    .bearer_auth(&creds.access_token)
                    .header("developer-token", creds.developer_token.as_str());
                if !creds.login_customer_id.trim().is_empty() {
                    builder = builder.header("login-customer-id", creds.login_customer_id.as_str());
                }
                let resp = builder
                    .json(&json!({ "query": gaql }))
                    .send()
                    .await
                    .map_err(|e| ConnectorError::Transient {
                        platform: PlatformId::GoogleAds,
                        message: e.to_string(),
                    })?;
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if !status.is_success() {
                    return Err(map_http_status(PlatformId::GoogleAds, status, &text, None));
                }
                serde_json::from_str(&text).map_err(|e| ConnectorError::ParseError {
                    platform: PlatformId::GoogleAds,
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
            let gross = Fen::from_yuan_f64(8600.0);
            let cash = gross.scale(account.default_cash_ratio);
            out.push(DailyFundStatement {
                platform: PlatformId::GoogleAds,
                ad_account_id: account.id.clone(),
                platform_account_id: account.platform_account_id.clone(),
                stat_date: cur,
                cash_balance_fen: Fen(450_000_00),
                grant_balance_fen: Fen(18_000_00),
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
                platform: PlatformId::GoogleAds,
                ad_account_id: account.id.clone(),
                affected_entity_ids: request.entity_ids.clone(),
                summary: format!("Google Ads v18 {:?} applied ({})", request.action, self.mode_label()),
                guardrail_notes: vec![],
                platform_response: json!({"results": []}),
            });
        }
        if let ConnectorMode::Live(creds) = &self.mode {
            let cid = Self::clean_customer_id(&account.platform_account_id);
            let campaign_id = request.entity_ids.first().cloned().unwrap_or_default();
            let status_val = match &request.action {
                AdMutationAction::Pause => "PAUSED",
                AdMutationAction::Resume => "ENABLED",
                _ => {
                    return Err(ConnectorError::InvalidRequest {
                        platform: PlatformId::GoogleAds,
                        message: "campaigns:mutate status update supports Pause/Resume; budget edits target CampaignBudgetService".into(),
                    })
                }
            };
            let url = format!("{}/customers/{cid}/campaigns:mutate", self.base_url());
            let mut builder = self
                .client
                .post(&url)
                .bearer_auth(&creds.access_token)
                .header("developer-token", creds.developer_token.as_str());
            if !creds.login_customer_id.trim().is_empty() {
                builder = builder.header("login-customer-id", creds.login_customer_id.as_str());
            }
            let resp = builder
                .json(&json!({
                    "operations": [{
                        "update": {
                            "resourceName": format!("customers/{cid}/campaigns/{campaign_id}"),
                            "status": status_val
                        },
                        "updateMask": "status"
                    }]
                }))
                .send()
                .await
                .map_err(|e| ConnectorError::Transient {
                    platform: PlatformId::GoogleAds,
                    message: e.to_string(),
                })?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(map_http_status(PlatformId::GoogleAds, status, &text, None));
            }
            return Ok(AdMutationResult {
                dry_run: false,
                accepted: true,
                platform: PlatformId::GoogleAds,
                ad_account_id: account.id.clone(),
                affected_entity_ids: request.entity_ids.clone(),
                summary: "Google Ads v18 live mutation submitted".into(),
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
        let cid = Self::clean_customer_id(&account.platform_account_id);
        let status_val = if request.initial_status.eq_ignore_ascii_case("PAUSED") {
            "PAUSED"
        } else {
            "ENABLED"
        };
        let (path, mut body) = match request.entity_kind {
            AdCreationEntityKind::Campaign => {
                let mut create_obj = json!({
                    "name": request.name,
                    "advertisingChannelType": request.marketing_goal.clone().unwrap_or_else(|| "PERFORMANCE_MAX".into()),
                    "status": status_val,
                });
                if let Some(b_fen) = request.daily_budget_fen {
                    create_obj["campaignBudgetAmountMicros"] =
                        json!(base_fen_to_currency_micros(b_fen, fx_rate).to_string());
                }
                (
                    format!("/customers/{cid}/campaigns:mutate"),
                    json!({ "operations": [{ "create": create_obj }] }),
                )
            }
            AdCreationEntityKind::AdGroup => {
                let camp_id = request.parent_entity_id.clone().ok_or_else(|| {
                    ConnectorError::InvalidRequest {
                        platform: PlatformId::GoogleAds,
                        message: "Google Ads adGroup creation requires parent_entity_id (campaign_id)".into(),
                    }
                })?;
                let mut create_obj = json!({
                    "name": request.name,
                    "campaign": format!("customers/{cid}/campaigns/{camp_id}"),
                    "status": status_val,
                });
                if let Some(bid_fen) = request.bid_fen {
                    create_obj["cpcBidMicros"] =
                        json!(base_fen_to_currency_micros(bid_fen, fx_rate).to_string());
                }
                (
                    format!("/customers/{cid}/adGroups:mutate"),
                    json!({ "operations": [{ "create": create_obj }] }),
                )
            }
            AdCreationEntityKind::Creative => {
                let ag_id = request.parent_entity_id.clone().ok_or_else(|| {
                    ConnectorError::InvalidRequest {
                        platform: PlatformId::GoogleAds,
                        message: "Google Ads adGroupAd creation requires parent_entity_id (adgroup_id)".into(),
                    }
                })?;
                (
                    format!("/customers/{cid}/adGroupAds:mutate"),
                    json!({
                        "operations": [{
                            "create": {
                                "adGroup": format!("customers/{cid}/adGroups/{ag_id}"),
                                "status": status_val,
                                "ad": {
                                    "name": request.name,
                                    "finalUrls": request.landing_url.iter().cloned().collect::<Vec<_>>(),
                                    "responsiveSearchAd": request.creative_assets
                                }
                            }
                        }]
                    }),
                )
            }
        };
        merge_json_extra(&mut body, &request.platform_extra);
        let note = format!(
            "Google Ads API v18 uses MICROS (10^-6 {currency}, fx={fx_rate}) for `campaignBudgetAmountMicros` & `cpcBidMicros`."
        );
        let risk_warnings = platform_creation_notes(PlatformId::GoogleAds, Some(&note));

        if request.dry_run || matches!(self.mode, ConnectorMode::Mock) {
            let created_id = if request.dry_run {
                None
            } else {
                Some(format!(
                    "gads_new_{}",
                    &Uuid::new_v4().simple().to_string()[..8]
                ))
            };
            return Ok(AdCreationResult {
                dry_run: request.dry_run,
                accepted: true,
                platform: PlatformId::GoogleAds,
                ad_account_id: account.id.clone(),
                entity_kind: request.entity_kind,
                created_entity_id: created_id.clone(),
                initial_status: request.initial_status.clone(),
                summary: format!(
                    "Google Ads v18 {:?} creation {} ({})",
                    request.entity_kind,
                    if request.dry_run { "previewed" } else { "created in mock" },
                    self.mode_label()
                ),
                risk_warnings,
                guardrail_notes: vec![format!("Target endpoint: POST {}{}", self.base_url(), path)],
                platform_payload_preview: body,
                platform_response: json!({
                    "results": [{ "resourceName": created_id }]
                }),
            });
        }

        if let ConnectorMode::Live(creds) = &self.mode {
            let url = format!("{}{}", self.base_url(), path);
            let mut builder = self
                .client
                .post(&url)
                .bearer_auth(&creds.access_token)
                .header("developer-token", creds.developer_token.as_str());
            if !creds.login_customer_id.trim().is_empty() {
                builder = builder.header("login-customer-id", creds.login_customer_id.as_str());
            }
            let resp = builder
                .json(&body)
                .send()
                .await
                .map_err(|e| ConnectorError::Transient {
                    platform: PlatformId::GoogleAds,
                    message: e.to_string(),
                })?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(map_http_status(PlatformId::GoogleAds, status, &text, None));
            }
            let val: Value = serde_json::from_str(&text).unwrap_or(json!({"raw": text}));
            let created_id = val
                .get("results")
                .and_then(Value::as_array)
                .and_then(|arr| arr.first())
                .and_then(|r| r.get("resourceName"))
                .and_then(Value::as_str)
                .map(|rn| rn.rsplit('/').next().unwrap_or(rn).to_string());
            return Ok(AdCreationResult {
                dry_run: false,
                accepted: true,
                platform: PlatformId::GoogleAds,
                ad_account_id: account.id.clone(),
                entity_kind: request.entity_kind,
                created_entity_id: created_id,
                initial_status: request.initial_status.clone(),
                summary: format!("Google Ads v18 live {:?} created", request.entity_kind),
                risk_warnings,
                guardrail_notes: vec![],
                platform_payload_preview: body,
                platform_response: val,
            });
        }
        unreachable!()
    }
}

pub struct GoogleAdsPlugin {
    connector: Arc<GoogleAdsConnector>,
}

impl GoogleAdsPlugin {
    pub fn new(mode: ConnectorMode) -> Self {
        Self {
            connector: Arc::new(GoogleAdsConnector::new(mode)),
        }
    }
}

#[async_trait]
impl Plugin for GoogleAdsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "connector.google_ads".into(),
            name: "Google Ads Connector (API v18 GAQL)".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Official Google Ads API v18 connector with GAQL SearchStream and micros-exact FX normalization".into(),
            category: PluginCategory::Connector,
            provides: vec!["connector:google_ads".into()],
            requires: vec!["storage".into()],
            config_schema: json!({"type": "object"}),
        }
    }

    async fn mount(&self, ctx: &mut PluginContext) -> KernelResult<()> {
        let svc: Arc<dyn AdPlatformConnector> =
            Arc::clone(&self.connector) as Arc<dyn AdPlatformConnector>;
        ctx.provide_named_service(PlatformId::GoogleAds.as_str(), svc);
        Ok(())
    }
}
