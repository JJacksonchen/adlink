//! Xiaohongshu Juguang (`小红书聚光`) & Chengfeng (`小红书乘风`) official connectors.
//!
//! Official endpoints (`https://adapi.xiaohongshu.com`, Header: `Access-Token`):
//! - `POST /api/open/jg/account/balance/info`
//! - `POST /api/open/jg/data/report/offline/{account,campaign,unit,creative}`
//! - `POST /api/open/cf/data/report/offline/{campaign,creative}`
//! - `POST /api/open/jg/unit/update/status`

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
    build_http_client, extract_raw_map, json_i64, json_str, json_u64, map_http_status, ConnectorMode,
};
use crate::mock_data::mock_entities_for_account;
use crate::mock_generator::synthesize_chunk_metrics;
use crate::onboarding::build_onboard_guide;

pub struct XhsAdsConnector {
    platform: PlatformId,
    mode: ConnectorMode,
    client: Client,
}

impl XhsAdsConnector {
    pub fn new(platform: PlatformId, mode: ConnectorMode) -> Self {
        assert!(matches!(
            platform,
            PlatformId::XhsJuguang | PlatformId::XhsChengfeng
        ));
        Self {
            platform,
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
        "https://adapi.xiaohongshu.com"
    }

    /// Parses official Xiaohongshu Juguang/Chengfeng JSON report envelope.
    pub fn parse_official_reports_response(
        platform: PlatformId,
        payload: &Value,
        account: &AdAccount,
        granularity: ReportGranularity,
        level: ReportLevel,
        time_line: TimeLine,
    ) -> ConnectorResult<ReportFetchPage> {
        let success = payload
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let code = payload.get("code").and_then(Value::as_i64).unwrap_or(0);
        if !success || (code != 0 && code != 200) {
            let msg = payload
                .get("msg")
                .or_else(|| payload.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("xhs api error");
            if code == 429 || msg.contains("limit") {
                return Err(ConnectorError::RateLimited {
                    platform,
                    retry_after_ms: Some(1000),
                    message: msg.to_string(),
                });
            }
            if code == 401 || code == 403 {
                return Err(ConnectorError::AuthError {
                    platform,
                    message: msg.to_string(),
                });
            }
            return Err(ConnectorError::InvalidRequest {
                platform,
                message: format!("XHS code {code}: {msg}"),
            });
        }

        let data = payload.get("data").unwrap_or(payload);
        let list = data
            .get("data_list")
            .or_else(|| data.get("list"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let mut rows = Vec::with_capacity(list.len());
        for item in list {
            let ds = json_str(&item, "time");
            let date_str = if ds.is_empty() {
                json_str(&item, "date")
            } else {
                ds
            };
            let stat_date = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d").map_err(|e| {
                ConnectorError::ParseError {
                    platform,
                    message: format!("invalid date '{date_str}': {e}"),
                }
            })?;

            let entity_id = match level {
                ReportLevel::Account => account.platform_account_id.clone(),
                ReportLevel::Campaign => json_str(&item, "campaign_id"),
                ReportLevel::AdGroup => json_str(&item, "unit_id"),
                ReportLevel::Creative | ReportLevel::ContentOrder => {
                    let cid = json_str(&item, "creative_id");
                    if cid.is_empty() {
                        json_str(&item, "note_id")
                    } else {
                        cid
                    }
                }
            };
            let entity_name = {
                let n = json_str(&item, "creative_name");
                if !n.is_empty() {
                    n
                } else {
                    let u = json_str(&item, "unit_name");
                    if !u.is_empty() {
                        u
                    } else {
                        account.account_name.clone()
                    }
                }
            };

            // Juguang / Chengfeng `fee` & `deal_order_gmv` normalized to integer Fen.
            let gross_fen = Fen::from_fen(json_i64(&item, "fee"));
            let cash_fen = gross_fen.scale(account.default_cash_ratio);
            let gmv_fen = Fen::from_fen(json_i64(&item, "deal_order_gmv"));

            rows.push(UnifiedReportRow {
                platform,
                ad_account_id: account.id.clone(),
                platform_account_id: account.platform_account_id.clone(),
                stat_date,
                stat_hour: item.get("hour").and_then(Value::as_i64).map(|h| h as i32).unwrap_or(-1),
                granularity,
                level,
                entity_id: if entity_id.is_empty() {
                    account.platform_account_id.clone()
                } else {
                    entity_id
                },
                entity_name,
                parent_entity_id: item.get("campaign_id").and_then(Value::as_str).map(str::to_string),
                content_id: item.get("note_id").and_then(Value::as_str).map(str::to_string),
                time_line,
                gross_cost_fen: gross_fen,
                cash_cost_fen: cash_fen,
                grant_cost_fen: gross_fen - cash_fen,
                impressions: json_u64(&item, "impression"),
                clicks: json_u64(&item, "click"),
                conversions: json_u64(&item, "convert_cnt").max(json_u64(&item, "deal_order_num")),
                orders: json_u64(&item, "deal_order_num"),
                gmv_fen,
                interactions: json_u64(&item, "interaction"),
                new_follows: json_u64(&item, "follow_cnt"),
                raw_fields: extract_raw_map(&item),
                ..Default::default()
            });
        }

        let total = data.get("total_count").and_then(Value::as_u64);
        let has_more = data
            .get("has_more")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        Ok(ReportFetchPage {
            rows,
            has_more,
            total_count: total,
            next_cursor: None,
        })
    }

    fn build_mock_official_payload(&self, req: &ReportFetchRequest) -> Value {
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
                    "time": dt.format("%Y-%m-%d").to_string(),
                    "hour": if m.stat_hour >= 0 { json!(m.stat_hour) } else { Value::Null },
                    "advertiser_id": req.account.platform_account_id,
                    "campaign_id": m.parent_entity_id.clone().unwrap_or_else(|| "jg_camp_01".into()),
                    "unit_id": m.entity_id,
                    "creative_id": m.entity_id,
                    "creative_name": m.entity_name,
                    "note_id": m.content_id,
                    "fee": m.gross_cost_fen.as_fen(),
                    "impression": m.impressions,
                    "click": m.clicks,
                    "interaction": m.interactions,
                    "convert_cnt": m.conversions,
                    "deal_order_num": m.orders,
                    "deal_order_gmv": m.gmv_fen.as_fen(),
                    "follow_cnt": m.new_follows,
                })
            })
            .collect();
        json!({
            "code": 0,
            "success": true,
            "msg": "ok",
            "data": {
                "data_list": list,
                "total_count": total,
                "has_more": has_more
            }
        })
    }
}

#[async_trait]
impl AdPlatformConnector for XhsAdsConnector {
    fn platform_id(&self) -> PlatformId {
        self.platform
    }

    fn mode_label(&self) -> &'static str {
        match self.mode {
            ConnectorMode::Mock => "mock",
            ConnectorMode::Live(_) => "live",
        }
    }

    fn onboard_guide(&self) -> PlatformOnboardGuide {
        build_onboard_guide(self.platform, self.mode_label())
    }

    async fn fetch_reports_page(&self, req: &ReportFetchRequest) -> ConnectorResult<ReportFetchPage> {
        let payload = match &self.mode {
            ConnectorMode::Mock => self.build_mock_official_payload(req),
            ConnectorMode::Live(creds) => {
                let prefix = if self.platform == PlatformId::XhsChengfeng {
                    "/api/open/cf/data/report/offline"
                } else {
                    "/api/open/jg/data/report/offline"
                };
                let suffix = match req.level {
                    ReportLevel::Account => "account",
                    ReportLevel::Campaign => "campaign",
                    ReportLevel::AdGroup => "unit",
                    ReportLevel::Creative | ReportLevel::ContentOrder => "creative",
                };
                let url = format!("{}{}/{}", self.base_url(), prefix, suffix);
                let body = json!({
                    "advertiser_id": req.account.platform_account_id.parse::<i64>().unwrap_or(0),
                    "start_date": req.start_date.format("%Y-%m-%d").to_string(),
                    "end_date": req.end_date.format("%Y-%m-%d").to_string(),
                    "time_unit": if req.granularity == ReportGranularity::Hourly { "HOUR" } else { "DAY" },
                    "page_num": req.page,
                    "page_size": req.page_size,
                });
                let resp = self
                    .client
                    .post(&url)
                    .header("Access-Token", creds.access_token.as_str())
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| ConnectorError::Transient {
                        platform: self.platform,
                        message: e.to_string(),
                    })?;
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if !status.is_success() {
                    return Err(map_http_status(self.platform, status, &text, None));
                }
                serde_json::from_str(&text).map_err(|e| ConnectorError::ParseError {
                    platform: self.platform,
                    message: e.to_string(),
                })?
            }
        };

        Self::parse_official_reports_response(
            self.platform,
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
            let url = format!("{}/api/open/jg/account/balance/info", self.base_url());
            let resp = self
                .client
                .post(&url)
                .header("Access-Token", creds.access_token.as_str())
                .json(&json!({
                    "advertiser_id": account.platform_account_id.parse::<i64>().unwrap_or(0)
                }))
                .send()
                .await
                .map_err(|e| ConnectorError::Transient {
                    platform: self.platform,
                    message: e.to_string(),
                })?;
            let text = resp.text().await.unwrap_or_default();
            let val: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let data = val.get("data").unwrap_or(&val);
            let total_bal = json_i64(data, "total_balance");
            let cash_bal = json_i64(data, "cash_balance");
            let today_spend = json_i64(data, "today_spend");
            let cash_spend = Fen(today_spend).scale(account.default_cash_ratio).as_fen();
            return Ok(vec![DailyFundStatement {
                platform: self.platform,
                ad_account_id: account.id.clone(),
                platform_account_id: account.platform_account_id.clone(),
                stat_date: end_date,
                cash_balance_fen: Fen(cash_bal),
                grant_balance_fen: Fen((total_bal - cash_bal).max(0)),
                cash_cost_fen: Fen(cash_spend),
                grant_cost_fen: Fen(today_spend - cash_spend),
                shared_wallet_cost_fen: Fen::ZERO,
                income_fen: Fen::ZERO,
                raw_fields: extract_raw_map(data),
            }]);
        }

        let mut out = Vec::new();
        let mut cur = start_date;
        while cur <= end_date {
            let gross = Fen::from_yuan_f64(2800.0);
            let cash = gross.scale(account.default_cash_ratio);
            out.push(DailyFundStatement {
                platform: self.platform,
                ad_account_id: account.id.clone(),
                platform_account_id: account.platform_account_id.clone(),
                stat_date: cur,
                cash_balance_fen: Fen(88_000_00),
                grant_balance_fen: Fen(9_000_00),
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
                platform: self.platform,
                ad_account_id: account.id.clone(),
                affected_entity_ids: request.entity_ids.clone(),
                summary: format!(
                    "XHS {:?} applied ({})",
                    request.action,
                    self.mode_label()
                ),
                guardrail_notes: vec![],
                platform_response: json!({"code": 0, "success": true}),
            });
        }

        if let ConnectorMode::Live(creds) = &self.mode {
            let url = format!("{}/api/open/jg/unit/update/status", self.base_url());
            let status_code = match &request.action {
                AdMutationAction::Resume => 1,
                AdMutationAction::Pause => 2,
                _ => {
                    return Err(ConnectorError::InvalidRequest {
                        platform: self.platform,
                        message: "unit/update/status supports Pause/Resume; budget edits use unit/update".into(),
                    })
                }
            };
            let ids: Vec<i64> = request
                .entity_ids
                .iter()
                .filter_map(|s| s.parse::<i64>().ok())
                .collect();
            let resp = self
                .client
                .post(&url)
                .header("Access-Token", creds.access_token.as_str())
                .json(&json!({
                    "advertiser_id": account.platform_account_id.parse::<i64>().unwrap_or(0),
                    "unit_ids": ids,
                    "status": status_code
                }))
                .send()
                .await
                .map_err(|e| ConnectorError::Transient {
                    platform: self.platform,
                    message: e.to_string(),
                })?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(map_http_status(self.platform, status, &text, None));
            }
            return Ok(AdMutationResult {
                dry_run: false,
                accepted: true,
                platform: self.platform,
                ad_account_id: account.id.clone(),
                affected_entity_ids: request.entity_ids.clone(),
                summary: "XHS live status updated".into(),
                guardrail_notes: vec![],
                platform_response: serde_json::from_str(&text).unwrap_or(json!({"raw": text})),
            });
        }
        unreachable!()
    }
}

pub struct XhsPlugin {
    platform: PlatformId,
    connector: Arc<XhsAdsConnector>,
}

impl XhsPlugin {
    pub fn juguang(mode: ConnectorMode) -> Self {
        Self {
            platform: PlatformId::XhsJuguang,
            connector: Arc::new(XhsAdsConnector::new(PlatformId::XhsJuguang, mode)),
        }
    }

    pub fn chengfeng(mode: ConnectorMode) -> Self {
        Self {
            platform: PlatformId::XhsChengfeng,
            connector: Arc::new(XhsAdsConnector::new(PlatformId::XhsChengfeng, mode)),
        }
    }
}

#[async_trait]
impl Plugin for XhsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: format!("connector.{}", self.platform.as_str()),
            name: self.platform.display_name_en().into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: format!("Official XHS OpenAPI connector for {}", self.platform.display_name_zh()),
            category: PluginCategory::Connector,
            provides: vec![format!("connector:{}", self.platform.as_str())],
            requires: vec!["storage".into()],
            config_schema: json!({"type": "object"}),
        }
    }

    async fn mount(&self, ctx: &mut PluginContext) -> KernelResult<()> {
        let svc: Arc<dyn AdPlatformConnector> =
            Arc::clone(&self.connector) as Arc<dyn AdPlatformConnector>;
        ctx.provide_named_service(self.platform.as_str(), svc);
        Ok(())
    }
}
