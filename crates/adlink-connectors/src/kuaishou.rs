//! Kuaishou Magnetic Engine (`快手磁力引擎`) Marketing API v1 connector.
//!
//! Official endpoints (`https://ad.e.kuaishou.com/rest/openapi/v1`, Header: `Access-Token`):
//! - `POST /rest/openapi/v1/report/account_report`
//! - `POST /rest/openapi/v1/report/campaign_report`
//! - `POST /rest/openapi/v1/report/unit_report`
//! - `POST /rest/openapi/v1/report/creative_report`
//! - `POST /rest/openapi/v1/campaign/update/status`

use std::sync::Arc;

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
use chrono::NaiveDate;
use reqwest::Client;
use serde_json::{json, Value};

use crate::http_helpers::{
    build_http_client, extract_raw_map, json_f64, json_str, json_u64, map_http_status, ConnectorMode,
};
use crate::mock_data::mock_entities_for_account;
use crate::mock_generator::synthesize_chunk_metrics;
use crate::onboarding::build_onboard_guide;

pub struct KuaishouConnector {
    mode: ConnectorMode,
    client: Client,
}

impl KuaishouConnector {
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
        "https://ad.e.kuaishou.com/rest/openapi"
    }

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
                .unwrap_or("kuaishou api error");
            if code == 400004 || msg.contains("rate") {
                return Err(ConnectorError::RateLimited {
                    platform: PlatformId::Kuaishou,
                    retry_after_ms: Some(1000),
                    message: format!("KS code {code}: {msg}"),
                });
            }
            return Err(ConnectorError::InvalidRequest {
                platform: PlatformId::Kuaishou,
                message: format!("KS code {code}: {msg}"),
            });
        }

        let data = payload.get("data").unwrap_or(payload);
        let details = data
            .get("details")
            .or_else(|| data.get("list"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let mut rows = Vec::with_capacity(details.len());
        for item in details {
            let ds = json_str(&item, "stat_date");
            let date_part = ds.split_whitespace().next().unwrap_or(&ds);
            let stat_date = NaiveDate::parse_from_str(date_part, "%Y-%m-%d").map_err(|e| {
                ConnectorError::ParseError {
                    platform: PlatformId::Kuaishou,
                    message: format!("invalid stat_date '{ds}': {e}"),
                }
            })?;

            let entity_id = match level {
                ReportLevel::Account => account.platform_account_id.clone(),
                ReportLevel::Campaign => json_str(&item, "campaign_id"),
                ReportLevel::AdGroup => json_str(&item, "unit_id"),
                ReportLevel::Creative | ReportLevel::ContentOrder => json_str(&item, "creative_id"),
            };

            // Kuaishou `charge` & `event_pay_purchase_amount` are reported in Yuan (float) -> Fen.
            let gross_fen = Fen::from_yuan_f64(json_f64(&item, "charge"));
            let cash_fen = gross_fen.scale(account.default_cash_ratio);
            let gmv_fen = Fen::from_yuan_f64(json_f64(&item, "event_pay_purchase_amount"));

            let clicks = json_u64(&item, "photo_click").max(json_u64(&item, "click_count"));
            let impressions = json_u64(&item, "show").max(json_u64(&item, "exposure_count"));
            let conversions = json_u64(&item, "activation_count")
                .max(json_u64(&item, "submit_form_count"))
                .max(json_u64(&item, "event_pay_purchase_count"));

            rows.push(UnifiedReportRow {
                platform: PlatformId::Kuaishou,
                ad_account_id: account.id.clone(),
                platform_account_id: account.platform_account_id.clone(),
                stat_date,
                stat_hour: item.get("stat_hour").and_then(Value::as_i64).map(|h| h as i32).unwrap_or(-1),
                granularity,
                level,
                entity_id: if entity_id.is_empty() {
                    account.platform_account_id.clone()
                } else {
                    entity_id
                },
                entity_name: {
                    let n = json_str(&item, "creative_name");
                    if n.is_empty() {
                        account.account_name.clone()
                    } else {
                        n
                    }
                },
                parent_entity_id: item.get("campaign_id").and_then(Value::as_str).map(str::to_string),
                content_id: item.get("photo_id").and_then(Value::as_str).map(str::to_string),
                time_line,
                gross_cost_fen: gross_fen,
                cash_cost_fen: cash_fen,
                grant_cost_fen: gross_fen - cash_fen,
                impressions,
                clicks,
                conversions,
                leads: json_u64(&item, "submit_form_count"),
                orders: json_u64(&item, "event_pay_purchase_count"),
                gmv_fen,
                interactions: json_u64(&item, "like") + json_u64(&item, "comment"),
                video_plays: json_u64(&item, "play_3s"),
                raw_fields: extract_raw_map(&item),
                ..Default::default()
            });
        }

        let total_count = data.get("total_count").and_then(Value::as_u64);
        let has_more = data
            .get("has_more")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(ReportFetchPage {
            rows,
            has_more,
            total_count,
            next_cursor: None,
        })
    }

    fn build_mock_payload(req: &ReportFetchRequest) -> Value {
        let (items, has_more, total) = synthesize_chunk_metrics(
            &req.account,
            req.start_date,
            req.end_date,
            req.granularity,
            req.level,
            req.page,
            req.page_size,
        );
        let details: Vec<Value> = items
            .into_iter()
            .map(|(dt, m)| {
                json!({
                    "stat_date": dt.format("%Y-%m-%d").to_string(),
                    "stat_hour": if m.stat_hour >= 0 { json!(m.stat_hour) } else { Value::Null },
                    "advertiser_id": req.account.platform_account_id,
                    "campaign_id": m.parent_entity_id.clone().unwrap_or_else(|| "ks_camp_01".into()),
                    "unit_id": m.entity_id,
                    "creative_id": m.entity_id,
                    "creative_name": m.entity_name,
                    "photo_id": m.content_id,
                    "charge": m.gross_cost_fen.to_yuan_f64(),
                    "show": m.impressions,
                    "photo_click": m.clicks,
                    "submit_form_count": m.leads,
                    "event_pay_purchase_count": m.orders,
                    "event_pay_purchase_amount": m.gmv_fen.to_yuan_f64(),
                    "activation_count": m.conversions,
                    "like": m.interactions,
                    "play_3s": m.video_plays,
                })
            })
            .collect();
        json!({
            "code": 0,
            "message": "OK",
            "data": {
                "total_count": total,
                "has_more": has_more,
                "details": details
            }
        })
    }
}

#[async_trait]
impl AdPlatformConnector for KuaishouConnector {
    fn platform_id(&self) -> PlatformId {
        PlatformId::Kuaishou
    }

    fn mode_label(&self) -> &'static str {
        match self.mode {
            ConnectorMode::Mock => "mock",
            ConnectorMode::Live(_) => "live",
        }
    }

    fn onboard_guide(&self) -> PlatformOnboardGuide {
        build_onboard_guide(PlatformId::Kuaishou, self.mode_label())
    }

    async fn fetch_reports_page(&self, req: &ReportFetchRequest) -> ConnectorResult<ReportFetchPage> {
        let payload = match &self.mode {
            ConnectorMode::Mock => Self::build_mock_payload(req),
            ConnectorMode::Live(creds) => {
                let path = match req.level {
                    ReportLevel::Account => "/v1/report/account_report",
                    ReportLevel::Campaign => "/v1/report/campaign_report",
                    ReportLevel::AdGroup => "/v1/report/unit_report",
                    ReportLevel::Creative | ReportLevel::ContentOrder => "/v1/report/creative_report",
                };
                let url = format!("{}{}", self.base_url(), path);
                let resp = self
                    .client
                    .post(&url)
                    .header("Access-Token", creds.access_token.as_str())
                    .json(&json!({
                        "advertiser_id": req.account.platform_account_id.parse::<i64>().unwrap_or(0),
                        "start_date": req.start_date.format("%Y-%m-%d").to_string(),
                        "end_date": req.end_date.format("%Y-%m-%d").to_string(),
                        "temporal_granularity": if req.granularity == ReportGranularity::Hourly { "HOURLY" } else { "DAILY" },
                        "page": req.page,
                        "page_size": req.page_size,
                    }))
                    .send()
                    .await
                    .map_err(|e| ConnectorError::Transient {
                        platform: PlatformId::Kuaishou,
                        message: e.to_string(),
                    })?;
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if !status.is_success() {
                    return Err(map_http_status(PlatformId::Kuaishou, status, &text, None));
                }
                serde_json::from_str(&text).map_err(|e| ConnectorError::ParseError {
                    platform: PlatformId::Kuaishou,
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
            let gross = Fen::from_yuan_f64(4200.0);
            let cash = gross.scale(account.default_cash_ratio);
            out.push(DailyFundStatement {
                platform: PlatformId::Kuaishou,
                ad_account_id: account.id.clone(),
                platform_account_id: account.platform_account_id.clone(),
                stat_date: cur,
                cash_balance_fen: Fen(98_000_00),
                grant_balance_fen: Fen(16_000_00),
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
                platform: PlatformId::Kuaishou,
                ad_account_id: account.id.clone(),
                affected_entity_ids: request.entity_ids.clone(),
                summary: format!("Kuaishou {:?} applied", request.action),
                guardrail_notes: vec![],
                platform_response: json!({"code": 0}),
            });
        }
        if let ConnectorMode::Live(creds) = &self.mode {
            let url = format!("{}/v1/campaign/update/status", self.base_url());
            let put_status = match &request.action {
                AdMutationAction::Resume => 1,
                AdMutationAction::Pause => 2,
                _ => {
                    return Err(ConnectorError::InvalidRequest {
                        platform: PlatformId::Kuaishou,
                        message: "campaign/update/status supports Pause/Resume".into(),
                    })
                }
            };
            let resp = self
                .client
                .post(&url)
                .header("Access-Token", creds.access_token.as_str())
                .json(&json!({
                    "advertiser_id": account.platform_account_id.parse::<i64>().unwrap_or(0),
                    "campaign_ids": request.entity_ids,
                    "put_status": put_status,
                }))
                .send()
                .await
                .map_err(|e| ConnectorError::Transient {
                    platform: PlatformId::Kuaishou,
                    message: e.to_string(),
                })?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(map_http_status(PlatformId::Kuaishou, status, &text, None));
            }
            return Ok(AdMutationResult {
                dry_run: false,
                accepted: true,
                platform: PlatformId::Kuaishou,
                ad_account_id: account.id.clone(),
                affected_entity_ids: request.entity_ids.clone(),
                summary: "Kuaishou live status updated".into(),
                guardrail_notes: vec![],
                platform_response: serde_json::from_str(&text).unwrap_or(json!({"raw": text})),
            });
        }
        unreachable!()
    }
}

pub struct KuaishouPlugin {
    connector: Arc<KuaishouConnector>,
}

impl KuaishouPlugin {
    pub fn new(mode: ConnectorMode) -> Self {
        Self {
            connector: Arc::new(KuaishouConnector::new(mode)),
        }
    }
}

#[async_trait]
impl Plugin for KuaishouPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "connector.kuaishou".into(),
            name: "Kuaishou Magnetic Engine Connector".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Official Marketing API v1 connector for 快手磁力引擎".into(),
            category: PluginCategory::Connector,
            provides: vec!["connector:kuaishou".into()],
            requires: vec!["storage".into()],
            config_schema: json!({"type": "object"}),
        }
    }

    async fn mount(&self, ctx: &mut PluginContext) -> KernelResult<()> {
        let svc: Arc<dyn AdPlatformConnector> =
            Arc::clone(&self.connector) as Arc<dyn AdPlatformConnector>;
        ctx.provide_named_service(PlatformId::Kuaishou.as_str(), svc);
        Ok(())
    }
}
