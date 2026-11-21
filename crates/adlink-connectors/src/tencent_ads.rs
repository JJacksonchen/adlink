//! Tencent Ads (腾讯社交广告 / AMS Marketing API v3.0) connector plugin.
//!
//! Official endpoints:
//! - `GET https://api.e.qq.com/v3.0/daily_reports/get`
//! - `GET https://api.e.qq.com/v3.0/hourly_reports/get`
//! - `GET https://api.e.qq.com/v3.0/funds/get`
//! - `GET https://api.e.qq.com/v3.0/daily_balance_report/get`
//! - `GET https://api.e.qq.com/v3.0/adgroups/get`
//! - `POST https://api.e.qq.com/v3.0/adgroups/update`

use adlink_domain::{
    AdAccount, AdEntity, AdMutationAction, AdMutationRequest, AdMutationResult, DailyFundStatement,
    Fen, PlatformId, ReportGranularity, ReportLevel, TimeLine, UnifiedReportRow,
};
use adlink_engine::{
    AdPlatformConnector, ConnectorError, ConnectorResult, PlatformOnboardGuide, ReportFetchPage,
    ReportFetchRequest,
};
use adlink_kernel::{KernelResult, Plugin, PluginCategory, PluginContext, PluginManifest};
use async_trait::async_trait;
use chrono::{NaiveDate, Utc};
use reqwest::Client;
use serde_json::{json, Value};

use crate::http_helpers::{
    build_http_client, extract_raw_map, json_i64, json_str, json_u64, map_http_status, ConnectorMode,
};
use crate::mock_data::mock_entities_for_account;
use crate::mock_generator::synthesize_chunk_metrics;
use crate::onboarding::build_onboard_guide;

pub struct TencentAdsConnector {
    mode: ConnectorMode,
    client: Client,
}

impl TencentAdsConnector {
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
        "https://api.e.qq.com/v3.0"
    }

    /// Parses official Tencent AMS v3.0 `daily_reports/get` / `hourly_reports/get` JSON payload.
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
                .unwrap_or("unknown error");
            if code == 11000 || code == 11004 {
                return Err(ConnectorError::RateLimited {
                    platform: PlatformId::TencentAds,
                    retry_after_ms: Some(1000),
                    message: format!("AMS code {code}: {msg}"),
                });
            }
            if code == 10000 || code == 10001 {
                return Err(ConnectorError::AuthError {
                    platform: PlatformId::TencentAds,
                    message: format!("AMS code {code}: {msg}"),
                });
            }
            return Err(ConnectorError::InvalidRequest {
                platform: PlatformId::TencentAds,
                message: format!("AMS code {code}: {msg}"),
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
            let date_str = json_str(&item, "date");
            let stat_date = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d").map_err(|e| {
                ConnectorError::ParseError {
                    platform: PlatformId::TencentAds,
                    message: format!("invalid date '{date_str}': {e}"),
                }
            })?;
            let stat_hour = item
                .get("hour")
                .and_then(Value::as_i64)
                .map(|h| h as i32)
                .unwrap_or(-1);

            let entity_id = match level {
                ReportLevel::Account => json_str(&item, "account_id"),
                ReportLevel::Campaign => json_str(&item, "campaign_id"),
                ReportLevel::AdGroup => json_str(&item, "adgroup_id"),
                ReportLevel::Creative | ReportLevel::ContentOrder => {
                    let dc = json_str(&item, "dynamic_creative_id");
                    if dc.is_empty() {
                        json_str(&item, "adgroup_id")
                    } else {
                        dc
                    }
                }
            };
            let entity_name = {
                let n = json_str(&item, "adgroup_name");
                if !n.is_empty() {
                    n
                } else {
                    let c = json_str(&item, "campaign_name");
                    if !c.is_empty() {
                        c
                    } else {
                        account.account_name.clone()
                    }
                }
            };

            // AMS v3.0 `cost` is already in Fen (分).
            let gross_fen = Fen::from_fen(json_i64(&item, "cost"));
            let cash_fen = gross_fen.scale(account.default_cash_ratio);
            let grant_fen = gross_fen - cash_fen;
            let gmv_fen = Fen::from_fen(json_i64(&item, "cheout_fd"));

            let mut row = UnifiedReportRow {
                platform: PlatformId::TencentAds,
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
                parent_entity_id: item
                    .get("campaign_id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                content_id: item
                    .get("material_id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                time_line,
                gross_cost_fen: gross_fen,
                cash_cost_fen: cash_fen,
                grant_cost_fen: grant_fen,
                impressions: json_u64(&item, "view_count"),
                clicks: json_u64(&item, "valid_click_count"),
                conversions: json_u64(&item, "conversions_count"),
                deep_conversions: json_u64(&item, "deep_conversions_count"),
                leads: json_u64(&item, "web_leads_count"),
                orders: json_u64(&item, "cheout_fd_count"),
                gmv_fen,
                interactions: json_u64(&item, "praise_count") + json_u64(&item, "comment_count"),
                video_plays: json_u64(&item, "video_play_count"),
                effective_plays: json_u64(&item, "video_inner_play_count"),
                raw_fields: extract_raw_map(&item),
                ..Default::default()
            };
            if row.impressions == 0 {
                row.impressions = json_u64(&item, "impression");
            }
            rows.push(row);
        }

        let page_info = data.get("page_info").unwrap_or(&Value::Null);
        let total_page = page_info
            .get("total_page")
            .and_then(Value::as_u64)
            .unwrap_or(1);
        let current_page = page_info.get("page").and_then(Value::as_u64).unwrap_or(1);
        let total_number = page_info.get("total_number").and_then(Value::as_u64);

        Ok(ReportFetchPage {
            rows,
            has_more: current_page < total_page,
            total_count: total_number,
            next_cursor: None,
        })
    }

    fn build_mock_official_payload(req: &ReportFetchRequest) -> Value {
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
                json!({
                    "date": dt.format("%Y-%m-%d").to_string(),
                    "hour": if m.stat_hour >= 0 { json!(m.stat_hour) } else { Value::Null },
                    "account_id": req.account.platform_account_id,
                    "campaign_id": m.parent_entity_id.clone().unwrap_or_else(|| "tc_camp_01".into()),
                    "adgroup_id": m.entity_id,
                    "dynamic_creative_id": m.entity_id,
                    "adgroup_name": m.entity_name,
                    "material_id": m.content_id,
                    "time_line": match req.time_line {
                        TimeLine::ReportingTime => "REPORTING_TIME",
                        TimeLine::RequestTime => "REQUEST_TIME",
                    },
                    "cost": m.gross_cost_fen.as_fen(),
                    "view_count": m.impressions,
                    "valid_click_count": m.clicks,
                    "conversions_count": m.conversions,
                    "deep_conversions_count": m.deep_conversions,
                    "web_leads_count": m.leads,
                    "cheout_fd_count": m.orders,
                    "cheout_fd": m.gmv_fen.as_fen(),
                    "praise_count": m.interactions,
                    "video_play_count": m.video_plays,
                    "video_inner_play_count": m.effective_plays,
                })
            })
            .collect();

        let total_page = if has_more { req.page + 1 } else { req.page };
        json!({
            "code": 0,
            "message": "",
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
impl AdPlatformConnector for TencentAdsConnector {
    fn platform_id(&self) -> PlatformId {
        PlatformId::TencentAds
    }

    fn mode_label(&self) -> &'static str {
        match self.mode {
            ConnectorMode::Mock => "mock",
            ConnectorMode::Live(_) => "live",
        }
    }

    fn onboard_guide(&self) -> PlatformOnboardGuide {
        build_onboard_guide(PlatformId::TencentAds, self.mode_label())
    }

    async fn fetch_reports_page(&self, req: &ReportFetchRequest) -> ConnectorResult<ReportFetchPage> {
        let payload = match &self.mode {
            ConnectorMode::Mock => Self::build_mock_official_payload(req),
            ConnectorMode::Live(creds) => {
                let endpoint = if req.granularity == ReportGranularity::Hourly {
                    format!("{}/hourly_reports/get", self.base_url())
                } else {
                    format!("{}/daily_reports/get", self.base_url())
                };
                let level_str = match req.level {
                    ReportLevel::Account => "REPORT_LEVEL_ADVERTISER",
                    ReportLevel::Campaign => "REPORT_LEVEL_CAMPAIGN",
                    ReportLevel::AdGroup => "REPORT_LEVEL_ADGROUP",
                    ReportLevel::Creative | ReportLevel::ContentOrder => {
                        "REPORT_LEVEL_DYNAMIC_CREATIVE"
                    }
                };
                let tl_str = match req.time_line {
                    TimeLine::ReportingTime => "REPORTING_TIME",
                    TimeLine::RequestTime => "REQUEST_TIME",
                };
                let date_range = json!({
                    "start_date": req.start_date.format("%Y-%m-%d").to_string(),
                    "end_date": req.end_date.format("%Y-%m-%d").to_string(),
                })
                .to_string();
                let fields = json!([
                    "date",
                    "account_id",
                    "campaign_id",
                    "adgroup_id",
                    "cost",
                    "view_count",
                    "valid_click_count",
                    "conversions_count",
                    "deep_conversions_count",
                    "cheout_fd",
                    "cheout_fd_count"
                ])
                .to_string();

                let resp = self
                    .client
                    .get(&endpoint)
                    .query(&[
                        ("access_token", creds.access_token.as_str()),
                        ("timestamp", &Utc::now().timestamp().to_string()),
                        ("nonce", "adlink"),
                        ("account_id", req.account.platform_account_id.as_str()),
                        ("level", level_str),
                        ("date_range", &date_range),
                        ("fields", &fields),
                        ("time_line", tl_str),
                        ("page", &req.page.to_string()),
                        ("page_size", &req.page_size.to_string()),
                    ])
                    .send()
                    .await
                    .map_err(|e| ConnectorError::Transient {
                        platform: PlatformId::TencentAds,
                        message: e.to_string(),
                    })?;

                let status = resp.status();
                let retry_after = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|h| h.to_str().ok())
                    .map(str::to_string);
                let text = resp.text().await.map_err(|e| ConnectorError::Transient {
                    platform: PlatformId::TencentAds,
                    message: e.to_string(),
                })?;
                if !status.is_success() {
                    return Err(map_http_status(
                        PlatformId::TencentAds,
                        status,
                        &text,
                        retry_after.as_deref(),
                    ));
                }
                serde_json::from_str(&text).map_err(|e| ConnectorError::ParseError {
                    platform: PlatformId::TencentAds,
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
        if let ConnectorMode::Live(creds) = &self.mode {
            let endpoint = format!("{}/funds/get", self.base_url());
            let resp = self
                .client
                .get(&endpoint)
                .query(&[
                    ("access_token", creds.access_token.as_str()),
                    ("account_id", account.platform_account_id.as_str()),
                ])
                .send()
                .await
                .map_err(|e| ConnectorError::Transient {
                    platform: PlatformId::TencentAds,
                    message: e.to_string(),
                })?;
            let text = resp.text().await.map_err(|e| ConnectorError::Transient {
                platform: PlatformId::TencentAds,
                message: e.to_string(),
            })?;
            let json_val: Value =
                serde_json::from_str(&text).map_err(|e| ConnectorError::ParseError {
                    platform: PlatformId::TencentAds,
                    message: e.to_string(),
                })?;
            let mut cash_bal = 0i64;
            let mut grant_bal = 0i64;
            let mut cash_cost = 0i64;
            let mut grant_cost = 0i64;
            if let Some(list) = json_val
                .get("data")
                .and_then(|d| d.get("list"))
                .and_then(Value::as_array)
            {
                for item in list {
                    let ft = json_str(item, "fund_type");
                    let bal = json_i64(item, "balance");
                    let rt = json_i64(item, "realtime_cost");
                    if ft.contains("CASH") {
                        cash_bal += bal;
                        cash_cost += rt;
                    } else {
                        grant_bal += bal;
                        grant_cost += rt;
                    }
                }
            }
            return Ok(vec![DailyFundStatement {
                platform: PlatformId::TencentAds,
                ad_account_id: account.id.clone(),
                platform_account_id: account.platform_account_id.clone(),
                stat_date: end_date,
                cash_balance_fen: Fen(cash_bal),
                grant_balance_fen: Fen(grant_bal),
                cash_cost_fen: Fen(cash_cost),
                grant_cost_fen: Fen(grant_cost),
                shared_wallet_cost_fen: Fen::ZERO,
                income_fen: Fen::ZERO,
                raw_fields: extract_raw_map(&json_val),
            }]);
        }

        // Mock fund statement aligned with `default_cash_ratio`.
        let mut out = Vec::new();
        let mut cur = start_date;
        while cur <= end_date {
            let gross = Fen::from_yuan_f64(4900.0);
            let cash = gross.scale(account.default_cash_ratio);
            out.push(DailyFundStatement {
                platform: PlatformId::TencentAds,
                ad_account_id: account.id.clone(),
                platform_account_id: account.platform_account_id.clone(),
                stat_date: cur,
                cash_balance_fen: Fen(150_000_00),
                grant_balance_fen: Fen(25_000_00),
                cash_cost_fen: cash,
                grant_cost_fen: gross - cash,
                shared_wallet_cost_fen: Fen::ZERO,
                income_fen: Fen::ZERO,
                raw_fields: [("fund_types", json!(["GENERAL_CASH", "GENERAL_GIFT"]))]
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect(),
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
                platform: PlatformId::TencentAds,
                ad_account_id: account.id.clone(),
                affected_entity_ids: request.entity_ids.clone(),
                summary: format!(
                    "Tencent AMS v3.0 {:?} applied ({})",
                    request.action,
                    self.mode_label()
                ),
                guardrail_notes: vec![],
                platform_response: json!({"code": 0, "mock": matches!(self.mode, ConnectorMode::Mock)}),
            });
        }

        if let ConnectorMode::Live(creds) = &self.mode {
            let endpoint = format!("{}/adgroups/update", self.base_url());
            let mut body = json!({
                "account_id": account.platform_account_id,
                "adgroup_id": request.entity_ids.first().cloned().unwrap_or_default(),
            });
            match &request.action {
                AdMutationAction::Pause => {
                    body["configured_status"] = json!("AD_STATUS_SUSPEND");
                }
                AdMutationAction::Resume => {
                    body["configured_status"] = json!("AD_STATUS_NORMAL");
                }
                AdMutationAction::UpdateBudget { daily_budget_fen } => {
                    body["daily_budget"] = json!(daily_budget_fen);
                }
                AdMutationAction::UpdateBid { bid_fen } => {
                    body["bid_amount"] = json!(bid_fen);
                }
            }
            let resp = self
                .client
                .post(&endpoint)
                .query(&[("access_token", creds.access_token.as_str())])
                .json(&body)
                .send()
                .await
                .map_err(|e| ConnectorError::Transient {
                    platform: PlatformId::TencentAds,
                    message: e.to_string(),
                })?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(map_http_status(PlatformId::TencentAds, status, &text, None));
            }
            let val: Value = serde_json::from_str(&text).unwrap_or(json!({"raw": text}));
            return Ok(AdMutationResult {
                dry_run: false,
                accepted: true,
                platform: PlatformId::TencentAds,
                ad_account_id: account.id.clone(),
                affected_entity_ids: request.entity_ids.clone(),
                summary: "Tencent AMS v3.0 live mutation submitted".into(),
                guardrail_notes: vec![],
                platform_response: val,
            });
        }
        unreachable!()
    }
}

pub struct TencentAdsPlugin {
    connector: std::sync::Arc<TencentAdsConnector>,
}

impl TencentAdsPlugin {
    pub fn new(mode: ConnectorMode) -> Self {
        Self {
            connector: std::sync::Arc::new(TencentAdsConnector::new(mode)),
        }
    }
}

#[async_trait]
impl Plugin for TencentAdsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "connector.tencent_ads".into(),
            name: "Tencent Ads Connector (AMS v3.0)".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Official Marketing API v3.0 connector for 腾讯社交广告 (daily/hourly reports, funds, adgroups)".into(),
            category: PluginCategory::Connector,
            provides: vec!["connector:tencent_ads".into()],
            requires: vec!["storage".into()],
            config_schema: json!({"type": "object"}),
        }
    }

    async fn mount(&self, ctx: &mut PluginContext) -> KernelResult<()> {
        let svc: std::sync::Arc<dyn AdPlatformConnector> =
            std::sync::Arc::clone(&self.connector) as std::sync::Arc<dyn AdPlatformConnector>;
        ctx.provide_named_service(PlatformId::TencentAds.as_str(), svc);
        Ok(())
    }
}
