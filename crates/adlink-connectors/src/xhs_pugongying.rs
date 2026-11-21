//! Xiaohongshu Pugongying (`小红书蒲公英`) KOL collaboration & seeding connector.
//!
//! Honest API compliance: Pugongying does not expose a public self-serve MAPI.
//! AdLink provides a compliant hybrid connector supporting:
//! 1. Official whitelisted partner endpoint (if `base_url_override` + `access_token` supplied),
//! 2. Structured CSV/JSON ingestion (`POST /api/v1/ingest` & MCP `adlink_ingest_external_records`),
//! 3. Official-schema Mock mode linking `note_id` seamlessly with Juguang & Chengfeng.

use std::sync::Arc;

use adlink_domain::{
    AdAccount, AdEntity, AdMutationRequest, AdMutationResult, DailyFundStatement, Fen, PlatformId,
    ReportGranularity, ReportLevel, TimeLine, UnifiedReportRow,
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

pub struct XhsPugongyingConnector {
    mode: ConnectorMode,
    client: Client,
}

impl XhsPugongyingConnector {
    pub fn new(mode: ConnectorMode) -> Self {
        Self {
            mode,
            client: build_http_client(),
        }
    }

    pub fn parse_official_or_import_payload(
        payload: &Value,
        account: &AdAccount,
        granularity: ReportGranularity,
        level: ReportLevel,
        time_line: TimeLine,
    ) -> ConnectorResult<ReportFetchPage> {
        let list = payload
            .get("data")
            .and_then(|d| d.get("orders").or_else(|| d.get("list")))
            .or_else(|| payload.get("orders"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let mut rows = Vec::with_capacity(list.len());
        for item in list {
            let date_s = json_str(&item, "stat_date");
            let stat_date = NaiveDate::parse_from_str(&date_s, "%Y-%m-%d").map_err(|e| {
                ConnectorError::ParseError {
                    platform: PlatformId::XhsPugongying,
                    message: format!("invalid stat_date '{date_s}': {e}"),
                }
            })?;
            let order_id = json_str(&item, "order_id");
            let kol_fee = Fen::from_fen(json_i64(&item, "kol_quote_fee"));
            let svc_fee = Fen::from_fen(json_i64(&item, "platform_service_fee"));
            let interactions = json_u64(&item, "like_cnt")
                + json_u64(&item, "collect_cnt")
                + json_u64(&item, "comment_cnt");

            rows.push(UnifiedReportRow {
                platform: PlatformId::XhsPugongying,
                ad_account_id: account.id.clone(),
                platform_account_id: account.platform_account_id.clone(),
                stat_date,
                stat_hour: -1,
                granularity,
                level,
                entity_id: if order_id.is_empty() {
                    account.platform_account_id.clone()
                } else {
                    order_id
                },
                entity_name: json_str(&item, "order_name"),
                content_id: item.get("note_id").and_then(Value::as_str).map(str::to_string),
                time_line,
                gross_cost_fen: Fen::ZERO,
                cash_cost_fen: Fen::ZERO,
                kol_order_fee_fen: kol_fee,
                platform_service_fee_fen: svc_fee,
                impressions: json_u64(&item, "read_cnt"),
                clicks: json_u64(&item, "click_cnt"),
                conversions: json_u64(&item, "convert_cnt"),
                orders: json_u64(&item, "order_cnt"),
                gmv_fen: Fen::from_fen(json_i64(&item, "gmv_fen")),
                interactions,
                new_follows: json_u64(&item, "follow_cnt"),
                raw_fields: extract_raw_map(&item),
                ..Default::default()
            });
        }

        let has_more = payload
            .get("data")
            .and_then(|d| d.get("has_more"))
            .and_then(Value::as_bool)
            .unwrap_or(false);

        Ok(ReportFetchPage {
            rows,
            has_more,
            total_count: None,
            next_cursor: None,
        })
    }

    fn build_mock_payload(req: &ReportFetchRequest) -> Value {
        let (items, has_more, _) = synthesize_chunk_metrics(
            &req.account,
            req.start_date,
            req.end_date,
            req.granularity,
            req.level,
            req.page,
            req.page_size,
        );
        let orders: Vec<Value> = items
            .into_iter()
            .map(|(dt, m)| {
                let likes = (m.interactions as f64 * 0.50).round() as u64;
                let collects = (m.interactions as f64 * 0.35).round() as u64;
                let comments = m.interactions.saturating_sub(likes + collects);
                json!({
                    "stat_date": dt.format("%Y-%m-%d").to_string(),
                    "order_id": m.entity_id,
                    "order_name": m.entity_name,
                    "note_id": m.content_id,
                    "kol_user_id": "kol_xhs_9201",
                    "kol_quote_fee": m.kol_order_fee_fen.as_fen(),
                    "platform_service_fee": m.platform_service_fee_fen.as_fen(),
                    "read_cnt": m.impressions,
                    "click_cnt": m.clicks,
                    "like_cnt": likes,
                    "collect_cnt": collects,
                    "comment_cnt": comments,
                    "convert_cnt": m.conversions,
                    "order_cnt": m.orders,
                    "gmv_fen": m.gmv_fen.as_fen(),
                    "follow_cnt": m.new_follows,
                })
            })
            .collect();
        json!({ "code": 0, "data": { "orders": orders, "has_more": has_more } })
    }
}

#[async_trait]
impl AdPlatformConnector for XhsPugongyingConnector {
    fn platform_id(&self) -> PlatformId {
        PlatformId::XhsPugongying
    }

    fn mode_label(&self) -> &'static str {
        match self.mode {
            ConnectorMode::Mock => "mock",
            ConnectorMode::Live(_) => "live",
        }
    }

    fn onboard_guide(&self) -> PlatformOnboardGuide {
        build_onboard_guide(PlatformId::XhsPugongying, self.mode_label())
    }

    async fn fetch_reports_page(&self, req: &ReportFetchRequest) -> ConnectorResult<ReportFetchPage> {
        let payload = match &self.mode {
            ConnectorMode::Mock => Self::build_mock_payload(req),
            ConnectorMode::Live(creds) => {
                let Some(partner_base) = &creds.base_url_override else {
                    return Err(ConnectorError::InvalidRequest {
                        platform: PlatformId::XhsPugongying,
                        message: "XHS Pugongying requires whitelisted partner base_url_override OR structured ingestion via /api/v1/ingest".into(),
                    });
                };
                let url = format!("{partner_base}/pgy/orders/report");
                let resp = self
                    .client
                    .post(&url)
                    .header("Access-Token", creds.access_token.as_str())
                    .json(&json!({
                        "account_id": req.account.platform_account_id,
                        "start_date": req.start_date.format("%Y-%m-%d").to_string(),
                        "end_date": req.end_date.format("%Y-%m-%d").to_string(),
                        "page": req.page,
                        "page_size": req.page_size,
                    }))
                    .send()
                    .await
                    .map_err(|e| ConnectorError::Transient {
                        platform: PlatformId::XhsPugongying,
                        message: e.to_string(),
                    })?;
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if !status.is_success() {
                    return Err(map_http_status(PlatformId::XhsPugongying, status, &text, None));
                }
                serde_json::from_str(&text).map_err(|e| ConnectorError::ParseError {
                    platform: PlatformId::XhsPugongying,
                    message: e.to_string(),
                })?
            }
        };

        Self::parse_official_or_import_payload(
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
        _start_date: NaiveDate,
        end_date: NaiveDate,
    ) -> ConnectorResult<Vec<DailyFundStatement>> {
        Ok(vec![DailyFundStatement {
            platform: PlatformId::XhsPugongying,
            ad_account_id: account.id.clone(),
            platform_account_id: account.platform_account_id.clone(),
            stat_date: end_date,
            cash_balance_fen: Fen(120_000_00),
            grant_balance_fen: Fen::ZERO,
            cash_cost_fen: Fen(1045_00),
            grant_cost_fen: Fen::ZERO,
            shared_wallet_cost_fen: Fen::ZERO,
            income_fen: Fen::ZERO,
            raw_fields: Default::default(),
        }])
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
        _account: &AdAccount,
        _request: &AdMutationRequest,
    ) -> ConnectorResult<AdMutationResult> {
        Err(ConnectorError::InvalidRequest {
            platform: PlatformId::XhsPugongying,
            message: "Pugongying KOL orders do not support automated bid/budget mutation".into(),
        })
    }
}

pub struct XhsPugongyingPlugin {
    connector: Arc<XhsPugongyingConnector>,
}

impl XhsPugongyingPlugin {
    pub fn new(mode: ConnectorMode) -> Self {
        Self {
            connector: Arc::new(XhsPugongyingConnector::new(mode)),
        }
    }
}

#[async_trait]
impl Plugin for XhsPugongyingPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "connector.xhs_pugongying".into(),
            name: "Xiaohongshu Pugongying Connector".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Hybrid partner API + structured ingest connector for 小红书蒲公英 KOL orders".into(),
            category: PluginCategory::Connector,
            provides: vec!["connector:xhs_pugongying".into()],
            requires: vec!["storage".into()],
            config_schema: json!({"type": "object"}),
        }
    }

    async fn mount(&self, ctx: &mut PluginContext) -> KernelResult<()> {
        let svc: Arc<dyn AdPlatformConnector> =
            Arc::clone(&self.connector) as Arc<dyn AdPlatformConnector>;
        ctx.provide_named_service(PlatformId::XhsPugongying.as_str(), svc);
        Ok(())
    }
}
