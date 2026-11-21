//! Ocean Engine AD (`巨量引擎 AD v3.0`) & Ocean Engine Local (`巨量本地推 v3.0`) connectors.
//!
//! Official endpoints (`https://api.oceanengine.com`, Header: `Access-Token`):
//! - `GET /open_api/v3.0/report/custom/get/`
//! - `GET /open_api/v3.0/project/list/`
//! - `GET /open_api/v3.0/promotion/list/`
//! - `GET /open_api/2/advertiser/fund/daily_stat/`
//! - `GET /open_api/v3.0/local/report/project/get/`
//! - `GET /open_api/v3.0/local/report/promotion/get/`

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

pub struct OceanEngineConnector {
    platform: PlatformId,
    mode: ConnectorMode,
    client: Client,
}

impl OceanEngineConnector {
    pub fn new(platform: PlatformId, mode: ConnectorMode) -> Self {
        assert!(matches!(
            platform,
            PlatformId::OceanEngineAd | PlatformId::OceanEngineLocal
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
        "https://api.oceanengine.com"
    }

    pub fn parse_official_reports_response(
        platform: PlatformId,
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
                .unwrap_or("oceanengine api error");
            if code == 40100 || code == 50000 {
                return Err(ConnectorError::RateLimited {
                    platform,
                    retry_after_ms: Some(1000),
                    message: format!("OE code {code}: {msg}"),
                });
            }
            if code == 40105 || code == 40102 {
                return Err(ConnectorError::AuthError {
                    platform,
                    message: format!("OE code {code}: {msg}"),
                });
            }
            return Err(ConnectorError::InvalidRequest {
                platform,
                message: format!("OE code {code}: {msg}"),
            });
        }

        let data = payload.get("data").unwrap_or(payload);
        let list = data
            .get("rows")
            .or_else(|| data.get("list"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let mut rows = Vec::with_capacity(list.len());
        for item in list {
            let dims = item.get("dimensions").unwrap_or(&item);
            let mets = item.get("metrics").unwrap_or(&item);

            let date_raw = json_str(dims, "stat_time_day");
            let date_only = date_raw.split_whitespace().next().unwrap_or(&date_raw);
            let stat_date = NaiveDate::parse_from_str(date_only, "%Y-%m-%d").map_err(|e| {
                ConnectorError::ParseError {
                    platform,
                    message: format!("invalid stat_time_day '{date_raw}': {e}"),
                }
            })?;

            let entity_id = match level {
                ReportLevel::Account => account.platform_account_id.clone(),
                ReportLevel::Campaign => json_str(dims, "project_id"),
                ReportLevel::AdGroup => json_str(dims, "promotion_id"),
                ReportLevel::Creative | ReportLevel::ContentOrder => {
                    let mid = json_str(dims, "material_id");
                    if mid.is_empty() {
                        json_str(dims, "promotion_id")
                    } else {
                        mid
                    }
                }
            };

            // Ocean Engine `stat_cost` and `pay_order_amount` are in Yuan -> convert to exact Fen.
            let gross_fen = Fen::from_yuan_f64(json_f64(mets, "stat_cost"));
            let cash_fen = gross_fen.scale(account.default_cash_ratio);
            let gmv_fen = Fen::from_yuan_f64(json_f64(mets, "pay_order_amount"));
            let content_id = dims
                .get("poi_id")
                .or_else(|| dims.get("material_id"))
                .and_then(Value::as_str)
                .map(str::to_string);

            let mut raw = extract_raw_map(dims);
            for (k, v) in extract_raw_map(mets) {
                raw.insert(k, v);
            }

            rows.push(UnifiedReportRow {
                platform,
                ad_account_id: account.id.clone(),
                platform_account_id: account.platform_account_id.clone(),
                stat_date,
                stat_hour: dims
                    .get("stat_time_hour")
                    .and_then(Value::as_i64)
                    .map(|h| h as i32)
                    .unwrap_or(-1),
                granularity,
                level,
                entity_id: if entity_id.is_empty() {
                    account.platform_account_id.clone()
                } else {
                    entity_id
                },
                entity_name: {
                    let n = json_str(dims, "promotion_name");
                    if !n.is_empty() {
                        n
                    } else {
                        let p = json_str(dims, "project_name");
                        if !p.is_empty() {
                            p
                        } else {
                            account.account_name.clone()
                        }
                    }
                },
                parent_entity_id: dims.get("project_id").and_then(Value::as_str).map(str::to_string),
                content_id,
                time_line,
                gross_cost_fen: gross_fen,
                cash_cost_fen: cash_fen,
                grant_cost_fen: gross_fen - cash_fen,
                impressions: json_u64(mets, "show_cnt"),
                clicks: json_u64(mets, "click_cnt"),
                conversions: json_u64(mets, "convert_cnt"),
                orders: json_u64(mets, "pay_order_count"),
                gmv_fen,
                interactions: json_u64(mets, "dy_like") + json_u64(mets, "dy_comment"),
                video_plays: json_u64(mets, "total_play"),
                effective_plays: json_u64(mets, "valid_play"),
                new_follows: json_u64(mets, "dy_follow"),
                store_verifies: json_u64(mets, "local_life_verify_cnt"),
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

    fn build_mock_payload(&self, req: &ReportFetchRequest) -> Value {
        let (items, has_more, total) = synthesize_chunk_metrics(
            &req.account,
            req.start_date,
            req.end_date,
            req.granularity,
            req.level,
            req.page,
            req.page_size,
        );
        let rows: Vec<Value> = items
            .into_iter()
            .map(|(dt, m)| {
                json!({
                    "dimensions": {
                        "stat_time_day": dt.format("%Y-%m-%d").to_string(),
                        "stat_time_hour": if m.stat_hour >= 0 { json!(m.stat_hour) } else { Value::Null },
                        "advertiser_id": req.account.platform_account_id,
                        "project_id": m.parent_entity_id.clone().unwrap_or_else(|| "oe_proj_01".into()),
                        "promotion_id": m.entity_id,
                        "promotion_name": m.entity_name,
                        "material_id": m.entity_id,
                        "poi_id": if self.platform == PlatformId::OceanEngineLocal { json!(m.content_id) } else { Value::Null },
                    },
                    "metrics": {
                        "stat_cost": m.gross_cost_fen.to_yuan_f64(),
                        "show_cnt": m.impressions,
                        "click_cnt": m.clicks,
                        "convert_cnt": m.conversions,
                        "pay_order_count": m.orders,
                        "pay_order_amount": m.gmv_fen.to_yuan_f64(),
                        "dy_like": m.interactions,
                        "total_play": m.video_plays,
                        "valid_play": m.effective_plays,
                        "dy_follow": m.new_follows,
                        "local_life_verify_cnt": m.store_verifies,
                    }
                })
            })
            .collect();

        let total_page = if has_more { req.page + 1 } else { req.page };
        json!({
            "code": 0,
            "message": "OK",
            "data": {
                "rows": rows,
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
impl AdPlatformConnector for OceanEngineConnector {
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
            ConnectorMode::Mock => self.build_mock_payload(req),
            ConnectorMode::Live(creds) => {
                let (url, query_pairs) = if self.platform == PlatformId::OceanEngineLocal {
                    let path = match req.level {
                        ReportLevel::Campaign | ReportLevel::Account => {
                            "/open_api/v3.0/local/report/project/get/"
                        }
                        _ => "/open_api/v3.0/local/report/promotion/get/",
                    };
                    let metrics = json!(["stat_cost", "show_cnt", "click_cnt", "convert_cnt", "pay_order_amount", "local_life_verify_cnt"]).to_string();
                    (
                        format!("{}{}", self.base_url(), path),
                        vec![
                            ("local_account_id", req.account.platform_account_id.clone()),
                            ("start_date", req.start_date.format("%Y-%m-%d").to_string()),
                            ("end_date", req.end_date.format("%Y-%m-%d").to_string()),
                            ("metrics", metrics),
                            ("page", req.page.to_string()),
                            ("page_size", req.page_size.to_string()),
                        ],
                    )
                } else {
                    let dims = match req.level {
                        ReportLevel::Account => json!(["stat_time_day", "advertiser_id"]),
                        ReportLevel::Campaign => json!(["stat_time_day", "project_id"]),
                        ReportLevel::AdGroup => json!(["stat_time_day", "promotion_id"]),
                        _ => json!(["stat_time_day", "promotion_id", "material_id"]),
                    };
                    let metrics = json!([
                        "stat_cost", "show_cnt", "click_cnt", "convert_cnt",
                        "pay_order_count", "pay_order_amount"
                    ]);
                    (
                        format!("{}/open_api/v3.0/report/custom/get/", self.base_url()),
                        vec![
                            ("advertiser_id", req.account.platform_account_id.clone()),
                            ("data_topic", "BASIC_DATA".into()),
                            ("dimensions", dims.to_string()),
                            ("metrics", metrics.to_string()),
                            ("start_time", format!("{} 00:00:00", req.start_date)),
                            ("end_time", format!("{} 23:59:59", req.end_date)),
                            ("page", req.page.to_string()),
                            ("page_size", req.page_size.to_string()),
                        ],
                    )
                };

                let resp = self
                    .client
                    .get(&url)
                    .header("Access-Token", creds.access_token.as_str())
                    .query(&query_pairs)
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
            let url = format!("{}/open_api/2/advertiser/fund/daily_stat/", self.base_url());
            let resp = self
                .client
                .get(&url)
                .header("Access-Token", creds.access_token.as_str())
                .query(&[
                    ("advertiser_id", account.platform_account_id.as_str()),
                    ("start_date", &start_date.format("%Y-%m-%d").to_string()),
                    ("end_date", &end_date.format("%Y-%m-%d").to_string()),
                ])
                .send()
                .await
                .map_err(|e| ConnectorError::Transient {
                    platform: self.platform,
                    message: e.to_string(),
                })?;
            let text = resp.text().await.unwrap_or_default();
            let val: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let mut out = Vec::new();
            if let Some(list) = val
                .get("data")
                .and_then(|d| d.get("list"))
                .and_then(Value::as_array)
            {
                for item in list {
                    let dt = NaiveDate::parse_from_str(&json_str(item, "date"), "%Y-%m-%d")
                        .unwrap_or(end_date);
                    let total_cost = Fen::from_yuan_f64(json_f64(item, "cost"));
                    let cash = total_cost.scale(account.default_cash_ratio);
                    out.push(DailyFundStatement {
                        platform: self.platform,
                        ad_account_id: account.id.clone(),
                        platform_account_id: account.platform_account_id.clone(),
                        stat_date: dt,
                        cash_balance_fen: Fen::from_yuan_f64(json_f64(item, "balance")),
                        grant_balance_fen: Fen::from_yuan_f64(json_f64(item, "grant_income")),
                        cash_cost_fen: cash,
                        grant_cost_fen: total_cost - cash,
                        shared_wallet_cost_fen: Fen::from_yuan_f64(json_f64(
                            item,
                            "shared_wallet_cost",
                        )),
                        income_fen: Fen::from_yuan_f64(json_f64(item, "income")),
                        raw_fields: extract_raw_map(item),
                    });
                }
            }
            return Ok(out);
        }

        let mut out = Vec::new();
        let mut cur = start_date;
        while cur <= end_date {
            let gross = Fen::from_yuan_f64(if self.platform == PlatformId::OceanEngineLocal {
                2500.0
            } else {
                6800.0
            });
            let cash = gross.scale(account.default_cash_ratio);
            out.push(DailyFundStatement {
                platform: self.platform,
                ad_account_id: account.id.clone(),
                platform_account_id: account.platform_account_id.clone(),
                stat_date: cur,
                cash_balance_fen: Fen(210_000_00),
                grant_balance_fen: Fen(32_000_00),
                cash_cost_fen: cash,
                grant_cost_fen: gross - cash,
                shared_wallet_cost_fen: Fen(500_00),
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
                summary: format!("Ocean Engine {:?} applied", request.action),
                guardrail_notes: vec![],
                platform_response: json!({"code": 0}),
            });
        }
        if let ConnectorMode::Live(creds) = &self.mode {
            let path = if self.platform == PlatformId::OceanEngineLocal {
                "/open_api/v3.0/local/promotion/status/update/"
            } else {
                "/open_api/v3.0/promotion/status/update/"
            };
            let status_val = match &request.action {
                AdMutationAction::Resume => "ENABLE",
                AdMutationAction::Pause => "DISABLE",
                _ => {
                    return Err(ConnectorError::InvalidRequest {
                        platform: self.platform,
                        message: "status/update supports Pause/Resume".into(),
                    })
                }
            };
            let url = format!("{}{}", self.base_url(), path);
            let resp = self
                .client
                .post(&url)
                .header("Access-Token", creds.access_token.as_str())
                .json(&json!({
                    "advertiser_id": account.platform_account_id.parse::<i64>().unwrap_or(0),
                    "data": request.entity_ids.iter().map(|id| json!({"promotion_id": id, "opt_status": status_val})).collect::<Vec<_>>()
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
                summary: "Ocean Engine live promotion status updated".into(),
                guardrail_notes: vec![],
                platform_response: serde_json::from_str(&text).unwrap_or(json!({"raw": text})),
            });
        }
        unreachable!()
    }
}

pub struct OceanEnginePlugin {
    platform: PlatformId,
    connector: Arc<OceanEngineConnector>,
}

impl OceanEnginePlugin {
    pub fn ad(mode: ConnectorMode) -> Self {
        Self {
            platform: PlatformId::OceanEngineAd,
            connector: Arc::new(OceanEngineConnector::new(PlatformId::OceanEngineAd, mode)),
        }
    }

    pub fn local(mode: ConnectorMode) -> Self {
        Self {
            platform: PlatformId::OceanEngineLocal,
            connector: Arc::new(OceanEngineConnector::new(PlatformId::OceanEngineLocal, mode)),
        }
    }
}

#[async_trait]
impl Plugin for OceanEnginePlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: format!("connector.{}", self.platform.as_str()),
            name: self.platform.display_name_en().into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: format!("Official Ocean Engine v3.0 connector for {}", self.platform.display_name_zh()),
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
