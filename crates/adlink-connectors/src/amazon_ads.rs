//! Amazon Ads API v3 (Sponsored Products / Brands / Display Reporting & Campaigns) connector.
//!
//! Official endpoints (`https://advertising-api.amazon.com`):
//! - Headers: `Authorization: Bearer {access_token}`, `Amazon-Ads-ClientId`, `Amazon-Advertising-API-Scope: {profile_id}`
//! - `POST /reporting/reports` (`adProduct=SPONSORED_PRODUCTS`, `reportTypeId=spCampaigns|spAdGroups|spAdvertisedProduct`)
//! - `GET /v2/profiles`
//! - Opt-in creation: `POST /sp/campaigns`, `POST /sp/adGroups`, `POST /sp/productAds`

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

pub struct AmazonAdsConnector {
    mode: ConnectorMode,
    client: Client,
}

impl AmazonAdsConnector {
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
        "https://advertising-api.amazon.com"
    }

    /// Parses downloaded Amazon Ads API v3 JSON report rows (or async status envelope).
    pub fn parse_official_reports_response(
        payload: &Value,
        account: &AdAccount,
        granularity: ReportGranularity,
        level: ReportLevel,
        time_line: TimeLine,
    ) -> ConnectorResult<ReportFetchPage> {
        if let Some(status_str) = payload.get("status").and_then(Value::as_str) {
            if status_str.eq_ignore_ascii_case("PENDING")
                || status_str.eq_ignore_ascii_case("PROCESSING")
            {
                let report_id = json_str(payload, "reportId");
                return Err(ConnectorError::AsyncTaskPending {
                    platform: PlatformId::AmazonAds,
                    task_id: report_id,
                });
            }
        }
        if let Some(msg) = payload.get("message").and_then(Value::as_str) {
            if !msg.is_empty() && payload.get("rows").is_none() && !payload.is_array() {
                return Err(ConnectorError::InvalidRequest {
                    platform: PlatformId::AmazonAds,
                    message: format!("Amazon Ads error: {msg}"),
                });
            }
        }

        let list = if let Some(arr) = payload.as_array() {
            arr.clone()
        } else {
            payload
                .get("rows")
                .or_else(|| payload.get("data"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };

        let mut rows = Vec::with_capacity(list.len());
        for item in list {
            let date_str = json_str(&item, "date");
            let stat_date = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d").map_err(|e| {
                ConnectorError::ParseError {
                    platform: PlatformId::AmazonAds,
                    message: format!("invalid date '{date_str}': {e}"),
                }
            })?;

            let (currency, fx_rate) = resolve_account_fx(
                account,
                item.get("campaignBudgetCurrencyCode")
                    .or_else(|| item.get("currency"))
                    .and_then(Value::as_str),
            );
            let cost_major = json_f64(&item, "cost");
            let gross_fen = Fen::from_yuan_f64(cost_major * fx_rate);
            let cash_fen = gross_fen.scale(account.default_cash_ratio);

            let sales_major = json_f64(&item, "sales14d").max(json_f64(&item, "sales7d"));
            let gmv_fen = Fen::from_yuan_f64(sales_major * fx_rate);
            let purchases = json_u64(&item, "purchases14d").max(json_u64(&item, "purchases7d"));

            let entity_id = match level {
                ReportLevel::Account => account.platform_account_id.clone(),
                ReportLevel::Campaign => json_str(&item, "campaignId"),
                ReportLevel::AdGroup => json_str(&item, "adGroupId"),
                ReportLevel::Creative | ReportLevel::ContentOrder => {
                    let ad_id = json_str(&item, "adId");
                    if ad_id.is_empty() {
                        json_str(&item, "advertisedAsin")
                    } else {
                        ad_id
                    }
                }
            };
            let entity_name = {
                let ag = json_str(&item, "adGroupName");
                if !ag.is_empty() {
                    ag
                } else {
                    let cn = json_str(&item, "campaignName");
                    if cn.is_empty() {
                        account.account_name.clone()
                    } else {
                        cn
                    }
                }
            };

            let mut raw = extract_raw_map(&item);
            stamp_currency_raw_fields(&mut raw, &currency, fx_rate, cost_major);

            rows.push(UnifiedReportRow {
                platform: PlatformId::AmazonAds,
                ad_account_id: account.id.clone(),
                platform_account_id: account.platform_account_id.clone(),
                stat_date,
                stat_hour: -1,
                granularity,
                level,
                entity_id: if entity_id.is_empty() {
                    account.platform_account_id.clone()
                } else {
                    entity_id
                },
                entity_name,
                parent_entity_id: json_opt_str(&item, "campaignId")
                    .or_else(|| json_opt_str(&item, "adGroupId")),
                content_id: json_opt_str(&item, "advertisedAsin")
                    .or_else(|| json_opt_str(&item, "advertisedSku")),
                time_line,
                gross_cost_fen: gross_fen,
                cash_cost_fen: cash_fen,
                grant_cost_fen: gross_fen - cash_fen,
                impressions: json_u64(&item, "impressions"),
                clicks: json_u64(&item, "clicks"),
                conversions: purchases,
                orders: purchases,
                gmv_fen,
                raw_fields: raw,
                ..Default::default()
            });
        }

        let total_count = Some(rows.len() as u64);
        Ok(ReportFetchPage {
            rows,
            has_more: false,
            total_count,
            next_cursor: None,
        })
    }

    fn build_mock_payload(req: &ReportFetchRequest) -> Value {
        let (currency, fx_rate) = resolve_account_fx(&req.account, None);
        let (items, _, _) = synthesize_chunk_metrics(
            &req.account,
            req.start_date,
            req.end_date,
            ReportGranularity::Daily,
            req.level,
            1,
            req.page_size.max(500),
        );
        let arr: Vec<Value> = items
            .into_iter()
            .map(|(dt, m)| {
                let cost_usd = ((m.gross_cost_fen.to_yuan_f64() / fx_rate) * 100.0).round() / 100.0;
                let sales_usd = ((m.gmv_fen.to_yuan_f64() / fx_rate) * 100.0).round() / 100.0;
                json!({
                    "date": dt.format("%Y-%m-%d").to_string(),
                    "campaignBudgetCurrencyCode": currency,
                    "campaignId": m.parent_entity_id.clone().unwrap_or_else(|| "amzn_camp_01".into()),
                    "campaignName": m.entity_name,
                    "adGroupId": m.entity_id,
                    "adGroupName": m.entity_name,
                    "adId": m.entity_id,
                    "advertisedAsin": m.content_id.unwrap_or_else(|| "B0CM9X8Y1Z".into()),
                    "impressions": m.impressions,
                    "clicks": m.clicks,
                    "cost": cost_usd,
                    "purchases14d": m.orders,
                    "unitsSoldClicks14d": m.orders,
                    "sales14d": sales_usd,
                })
            })
            .collect();
        Value::Array(arr)
    }
}

#[async_trait]
impl AdPlatformConnector for AmazonAdsConnector {
    fn platform_id(&self) -> PlatformId {
        PlatformId::AmazonAds
    }

    fn mode_label(&self) -> &'static str {
        match self.mode {
            ConnectorMode::Mock => "mock",
            ConnectorMode::Live(_) => "live",
        }
    }

    fn onboard_guide(&self) -> PlatformOnboardGuide {
        build_onboard_guide(PlatformId::AmazonAds, self.mode_label())
    }

    async fn discover_accounts(&self) -> ConnectorResult<Vec<DiscoveredPlatformAccount>> {
        match &self.mode {
            ConnectorMode::Mock => Ok(mock_discovered_accounts(
                PlatformId::AmazonAds,
                AuthLoginStrategy::OfficialMcpBridge,
            )),
            ConnectorMode::Live(creds) => {
                if let Some(bridged) =
                    discover_via_official_mcp_bridge(&self.client, PlatformId::AmazonAds, creds)
                        .await
                {
                    return Ok(bridged);
                }
                let url = format!("{}/v2/profiles", self.base_url());
                let resp = self
                    .client
                    .get(&url)
                    .bearer_auth(&creds.access_token)
                    .header("Amazon-Ads-ClientId", creds.app_id.as_str())
                    .send()
                    .await
                    .map_err(|e| ConnectorError::Transient {
                        platform: PlatformId::AmazonAds,
                        message: e.to_string(),
                    })?;
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if !status.is_success() {
                    return Err(map_http_status(PlatformId::AmazonAds, status, &text, None));
                }
                let val: Value =
                    serde_json::from_str(&text).map_err(|e| ConnectorError::ParseError {
                        platform: PlatformId::AmazonAds,
                        message: e.to_string(),
                    })?;
                let list = val.as_array().cloned().unwrap_or_default();
                Ok(list
                    .into_iter()
                    .filter_map(|item| {
                        let pid = json_str(&item, "profileId");
                        if pid.is_empty() {
                            return None;
                        }
                        let account_info = item.get("accountInfo").unwrap_or(&Value::Null);
                        let name = {
                            let n = json_str(account_info, "name");
                            if n.is_empty() {
                                format!("Amazon Profile {pid} ({})", json_str(&item, "countryCode"))
                            } else {
                                n
                            }
                        };
                        Some(DiscoveredPlatformAccount {
                            platform: PlatformId::AmazonAds,
                            platform_account_id: pid,
                            account_name: name,
                            account_type: AccountType::BrandDirect,
                            status: "active".into(),
                            parent_agent_id: json_opt_str(account_info, "id"),
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
                let url = format!("{}/reporting/reports", self.base_url());
                let (report_type_id, columns) = match req.level {
                    ReportLevel::AdGroup => (
                        "spAdGroups",
                        json!([
                            "date", "campaignBudgetCurrencyCode", "campaignId", "campaignName",
                            "adGroupId", "adGroupName", "impressions", "clicks", "cost",
                            "purchases14d", "sales14d"
                        ]),
                    ),
                    ReportLevel::Creative | ReportLevel::ContentOrder => (
                        "spAdvertisedProduct",
                        json!([
                            "date", "campaignBudgetCurrencyCode", "campaignId", "campaignName",
                            "adGroupId", "adGroupName", "adId", "advertisedAsin", "impressions",
                            "clicks", "cost", "purchases14d", "sales14d"
                        ]),
                    ),
                    _ => (
                        "spCampaigns",
                        json!([
                            "date", "campaignBudgetCurrencyCode", "campaignId", "campaignName",
                            "impressions", "clicks", "cost", "purchases14d", "sales14d"
                        ]),
                    ),
                };
                let scope = if creds.amazon_profile_id.is_empty() {
                    req.account.platform_account_id.as_str()
                } else {
                    creds.amazon_profile_id.as_str()
                };
                let resp = self
                    .client
                    .post(&url)
                    .bearer_auth(&creds.access_token)
                    .header("Amazon-Ads-ClientId", creds.app_id.as_str())
                    .header("Amazon-Advertising-API-Scope", scope)
                    .json(&json!({
                        "name": format!("AdLink {} {}", report_type_id, req.start_date),
                        "startDate": req.start_date.format("%Y-%m-%d").to_string(),
                        "endDate": req.end_date.format("%Y-%m-%d").to_string(),
                        "configuration": {
                            "adProduct": "SPONSORED_PRODUCTS",
                            "reportTypeId": report_type_id,
                            "timeUnit": "DAILY",
                            "format": "GZIP_JSON",
                            "columns": columns
                        }
                    }))
                    .send()
                    .await
                    .map_err(|e| ConnectorError::Transient {
                        platform: PlatformId::AmazonAds,
                        message: e.to_string(),
                    })?;
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if !status.is_success() {
                    return Err(map_http_status(PlatformId::AmazonAds, status, &text, None));
                }
                serde_json::from_str(&text).map_err(|e| ConnectorError::ParseError {
                    platform: PlatformId::AmazonAds,
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
            let gross = Fen::from_yuan_f64(4800.0);
            let cash = gross.scale(account.default_cash_ratio);
            out.push(DailyFundStatement {
                platform: PlatformId::AmazonAds,
                ad_account_id: account.id.clone(),
                platform_account_id: account.platform_account_id.clone(),
                stat_date: cur,
                cash_balance_fen: Fen(240_000_00),
                grant_balance_fen: Fen::ZERO,
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
                platform: PlatformId::AmazonAds,
                ad_account_id: account.id.clone(),
                affected_entity_ids: request.entity_ids.clone(),
                summary: format!("Amazon Ads v3 {:?} applied ({})", request.action, self.mode_label()),
                guardrail_notes: vec![],
                platform_response: json!({"campaigns": {"success": request.entity_ids}}),
            });
        }
        if let ConnectorMode::Live(creds) = &self.mode {
            let (_currency, fx_rate) = resolve_account_fx(account, None);
            let scope = if creds.amazon_profile_id.is_empty() {
                account.platform_account_id.as_str()
            } else {
                creds.amazon_profile_id.as_str()
            };
            let updates: Vec<Value> = request
                .entity_ids
                .iter()
                .map(|cid| match &request.action {
                    AdMutationAction::Pause => json!({ "campaignId": cid, "state": "PAUSED" }),
                    AdMutationAction::Resume => json!({ "campaignId": cid, "state": "ENABLED" }),
                    AdMutationAction::UpdateBudget { daily_budget_fen } => json!({
                        "campaignId": cid,
                        "budget": { "budget": base_fen_to_currency_major(*daily_budget_fen, fx_rate), "budgetType": "DAILY" }
                    }),
                    AdMutationAction::UpdateBid { .. } => json!({ "campaignId": cid }),
                })
                .collect();
            let url = format!("{}/sp/campaigns", self.base_url());
            let resp = self
                .client
                .put(&url)
                .bearer_auth(&creds.access_token)
                .header("Amazon-Ads-ClientId", creds.app_id.as_str())
                .header("Amazon-Advertising-API-Scope", scope)
                .json(&json!({ "campaigns": updates }))
                .send()
                .await
                .map_err(|e| ConnectorError::Transient {
                    platform: PlatformId::AmazonAds,
                    message: e.to_string(),
                })?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(map_http_status(PlatformId::AmazonAds, status, &text, None));
            }
            return Ok(AdMutationResult {
                dry_run: false,
                accepted: true,
                platform: PlatformId::AmazonAds,
                ad_account_id: account.id.clone(),
                affected_entity_ids: request.entity_ids.clone(),
                summary: "Amazon Ads v3 live mutation submitted".into(),
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
        let state_str = if request.initial_status.eq_ignore_ascii_case("PAUSED") {
            "PAUSED"
        } else {
            "ENABLED"
        };
        let (path, mut body) = match request.entity_kind {
            AdCreationEntityKind::Campaign => {
                let mut item = json!({
                    "name": request.name,
                    "targetingType": request.marketing_goal.clone().unwrap_or_else(|| "MANUAL".into()),
                    "state": state_str,
                });
                if let Some(b_fen) = request.daily_budget_fen {
                    item["budget"] = json!({
                        "budget": base_fen_to_currency_major(b_fen, fx_rate),
                        "budgetType": "DAILY"
                    });
                }
                ("/sp/campaigns", json!({ "campaigns": [item] }))
            }
            AdCreationEntityKind::AdGroup => {
                let cid = request.parent_entity_id.clone().ok_or_else(|| {
                    ConnectorError::InvalidRequest {
                        platform: PlatformId::AmazonAds,
                        message: "Amazon adGroup creation requires parent_entity_id (campaignId)".into(),
                    }
                })?;
                let mut item = json!({
                    "campaignId": cid,
                    "name": request.name,
                    "state": state_str,
                });
                if let Some(bid_fen) = request.bid_fen {
                    item["defaultBid"] = json!(base_fen_to_currency_major(bid_fen, fx_rate));
                }
                ("/sp/adGroups", json!({ "adGroups": [item] }))
            }
            AdCreationEntityKind::Creative => {
                let ag_id = request.parent_entity_id.clone().ok_or_else(|| {
                    ConnectorError::InvalidRequest {
                        platform: PlatformId::AmazonAds,
                        message: "Amazon productAd creation requires parent_entity_id (adGroupId)".into(),
                    }
                })?;
                let mut item = json!({
                    "adGroupId": ag_id,
                    "state": state_str,
                });
                merge_json_extra(&mut item, &request.creative_assets);
                ("/sp/productAds", json!({ "productAds": [item] }))
            }
        };
        merge_json_extra(&mut body, &request.platform_extra);
        let note = format!(
            "Amazon Ads v3 creation requires `Amazon-Advertising-API-Scope` profile ID and marketplace currency ({currency}, fx={fx_rate})."
        );
        let risk_warnings = platform_creation_notes(PlatformId::AmazonAds, Some(&note));

        if request.dry_run || matches!(self.mode, ConnectorMode::Mock) {
            let created_id = if request.dry_run {
                None
            } else {
                Some(format!(
                    "amzn_new_{}",
                    &Uuid::new_v4().simple().to_string()[..8]
                ))
            };
            return Ok(AdCreationResult {
                dry_run: request.dry_run,
                accepted: true,
                platform: PlatformId::AmazonAds,
                ad_account_id: account.id.clone(),
                entity_kind: request.entity_kind,
                created_entity_id: created_id.clone(),
                initial_status: request.initial_status.clone(),
                summary: format!(
                    "Amazon Ads v3 {:?} creation {} ({})",
                    request.entity_kind,
                    if request.dry_run { "previewed" } else { "created in mock" },
                    self.mode_label()
                ),
                risk_warnings,
                guardrail_notes: vec![format!("Target endpoint: POST {}{}", self.base_url(), path)],
                platform_payload_preview: body,
                platform_response: json!({ "success": [created_id] }),
            });
        }

        if let ConnectorMode::Live(creds) = &self.mode {
            let url = format!("{}{}", self.base_url(), path);
            let scope = if creds.amazon_profile_id.is_empty() {
                account.platform_account_id.as_str()
            } else {
                creds.amazon_profile_id.as_str()
            };
            let resp = self
                .client
                .post(&url)
                .bearer_auth(&creds.access_token)
                .header("Amazon-Ads-ClientId", creds.app_id.as_str())
                .header("Amazon-Advertising-API-Scope", scope)
                .json(&body)
                .send()
                .await
                .map_err(|e| ConnectorError::Transient {
                    platform: PlatformId::AmazonAds,
                    message: e.to_string(),
                })?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(map_http_status(PlatformId::AmazonAds, status, &text, None));
            }
            let val: Value = serde_json::from_str(&text).unwrap_or(json!({"raw": text}));
            let created_id = json_opt_str(&val, "campaignId")
                .or_else(|| json_opt_str(&val, "adGroupId"))
                .or_else(|| json_opt_str(&val, "adId"));
            return Ok(AdCreationResult {
                dry_run: false,
                accepted: true,
                platform: PlatformId::AmazonAds,
                ad_account_id: account.id.clone(),
                entity_kind: request.entity_kind,
                created_entity_id: created_id,
                initial_status: request.initial_status.clone(),
                summary: format!("Amazon Ads v3 live {:?} created", request.entity_kind),
                risk_warnings,
                guardrail_notes: vec![],
                platform_payload_preview: body,
                platform_response: val,
            });
        }
        unreachable!()
    }
}

pub struct AmazonAdsPlugin {
    connector: Arc<AmazonAdsConnector>,
}

impl AmazonAdsPlugin {
    pub fn new(mode: ConnectorMode) -> Self {
        Self {
            connector: Arc::new(AmazonAdsConnector::new(mode)),
        }
    }
}

#[async_trait]
impl Plugin for AmazonAdsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "connector.amazon_ads".into(),
            name: "Amazon Ads Connector (v3)".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Official Amazon Ads API v3 connector for Sponsored Products/Brands/Display".into(),
            category: PluginCategory::Connector,
            provides: vec!["connector:amazon_ads".into()],
            requires: vec!["storage".into()],
            config_schema: json!({"type": "object"}),
        }
    }

    async fn mount(&self, ctx: &mut PluginContext) -> KernelResult<()> {
        let svc: Arc<dyn AdPlatformConnector> =
            Arc::clone(&self.connector) as Arc<dyn AdPlatformConnector>;
        ctx.provide_named_service(PlatformId::AmazonAds.as_str(), svc);
        Ok(())
    }
}
