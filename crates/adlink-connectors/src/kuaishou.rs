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
    build_http_client, discover_via_official_mcp_bridge, extract_raw_map, json_f64, json_opt_str,
    json_str, json_u64, map_http_status, merge_json_extra, mock_discovered_accounts,
    platform_creation_notes, ConnectorMode,
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
        page: u32,
        page_size: u32,
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

            let entity_name = match level {
                ReportLevel::Account => account.account_name.clone(),
                ReportLevel::Campaign => {
                    let c = json_str(&item, "campaign_name");
                    if c.is_empty() {
                        account.account_name.clone()
                    } else {
                        c
                    }
                }
                ReportLevel::AdGroup => {
                    let u = json_str(&item, "unit_name");
                    if !u.is_empty() {
                        u
                    } else {
                        let c = json_str(&item, "campaign_name");
                        if c.is_empty() {
                            account.account_name.clone()
                        } else {
                            c
                        }
                    }
                }
                ReportLevel::Creative | ReportLevel::ContentOrder => {
                    let n = json_str(&item, "creative_name");
                    if !n.is_empty() {
                        n
                    } else {
                        let u = json_str(&item, "unit_name");
                        if !u.is_empty() {
                            u
                        } else {
                            let c = json_str(&item, "campaign_name");
                            if c.is_empty() {
                                account.account_name.clone()
                            } else {
                                c
                            }
                        }
                    }
                }
            };

            // Kuaishou `charge` & `event_pay_purchase_amount` are reported in Yuan (Double) -> Fen.
            let gross_fen = Fen::from_yuan_f64(json_f64(&item, "charge"));
            let cash_fen = gross_fen.scale(account.default_cash_ratio);
            let gmv_fen = Fen::from_yuan_f64(json_f64(&item, "event_pay_purchase_amount"));

            let clicks = json_u64(&item, "photo_click")
                .max(json_u64(&item, "aclick"))
                .max(json_u64(&item, "click_count"));
            let impressions = json_u64(&item, "show").max(json_u64(&item, "exposure_count"));
            let leads = json_u64(&item, "form_count")
                .max(json_u64(&item, "submit"))
                .max(json_u64(&item, "submit_form_count"));
            let orders = json_u64(&item, "event_pay").max(json_u64(&item, "event_pay_purchase_count"));
            let conversions = json_u64(&item, "activation")
                .max(json_u64(&item, "activation_count"))
                .max(leads)
                .max(orders);

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
                entity_name,
                parent_entity_id: json_opt_str(&item, "campaign_id"),
                content_id: json_opt_str(&item, "photo_id"),
                time_line,
                gross_cost_fen: gross_fen,
                cash_cost_fen: cash_fen,
                grant_cost_fen: gross_fen - cash_fen,
                impressions,
                clicks,
                conversions,
                leads,
                orders,
                gmv_fen,
                interactions: json_u64(&item, "like") + json_u64(&item, "comment"),
                video_plays: json_u64(&item, "play_3s"),
                raw_fields: extract_raw_map(&item),
                ..Default::default()
            });
        }

        let total_count = data.get("total_count").and_then(Value::as_u64);
        let fetched_through = u64::from(page.max(1)).saturating_mul(u64::from(page_size.max(1)));
        let has_more = data
            .get("has_more")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| total_count.is_some_and(|t| fetched_through < t && !rows.is_empty()));
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
                    "campaign_name": m.entity_name,
                    "unit_id": m.entity_id,
                    "unit_name": m.entity_name,
                    "creative_id": m.entity_id,
                    "creative_name": m.entity_name,
                    "photo_id": m.content_id,
                    "charge": m.gross_cost_fen.to_yuan_f64(),
                    "show": m.impressions,
                    "photo_click": m.clicks,
                    "aclick": m.clicks,
                    "form_count": m.leads,
                    "submit": m.leads,
                    "submit_form_count": m.leads,
                    "event_pay": m.orders,
                    "event_pay_purchase_count": m.orders,
                    "event_pay_purchase_amount": m.gmv_fen.to_yuan_f64(),
                    "activation": m.conversions,
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

    async fn discover_accounts(&self) -> ConnectorResult<Vec<DiscoveredPlatformAccount>> {
        match &self.mode {
            ConnectorMode::Mock => Ok(mock_discovered_accounts(
                PlatformId::Kuaishou,
                AuthLoginStrategy::OfficialMcpBridge,
            )),
            ConnectorMode::Live(creds) => {
                if let Some(bridged) =
                    discover_via_official_mcp_bridge(&self.client, PlatformId::Kuaishou, creds)
                        .await
                {
                    return Ok(bridged);
                }
                let url = format!("{}/v1/agent/advertiser/list", self.base_url());
                let resp = self
                    .client
                    .post(&url)
                    .header("Access-Token", creds.access_token.as_str())
                    .json(&json!({ "page": 1, "page_size": 100 }))
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
                let val: Value =
                    serde_json::from_str(&text).map_err(|e| ConnectorError::ParseError {
                        platform: PlatformId::Kuaishou,
                        message: e.to_string(),
                    })?;
                let list = val
                    .get("data")
                    .and_then(|d| d.get("details").or_else(|| d.get("list")))
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
                            let n = json_str(&item, "advertiser_name")
                                .if_empty_str(|| json_str(&item, "corporation_name"));
                            if n.is_empty() {
                                format!("Kuaishou {pid}")
                            } else {
                                n
                            }
                        };
                        Some(DiscoveredPlatformAccount {
                            platform: PlatformId::Kuaishou,
                            platform_account_id: pid,
                            account_name: name,
                            account_type: AccountType::AgencySub,
                            status: "active".into(),
                            parent_agent_id: json_opt_str(&item, "agent_id"),
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
            req.page,
            req.page_size,
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

    async fn create_ad_entity(
        &self,
        account: &AdAccount,
        request: &AdCreationRequest,
    ) -> ConnectorResult<AdCreationResult> {
        let put_status = if request.initial_status.eq_ignore_ascii_case("PAUSED") {
            2
        } else {
            1
        };
        let adv_id = account.platform_account_id.parse::<i64>().unwrap_or(0);
        // CRITICAL: Kuaishou creation APIs use `li` (厘 = 1/1000 Yuan = 0.1 Fen), so `fen * 10 = li`!
        let (path, mut body) = match request.entity_kind {
            AdCreationEntityKind::Campaign => {
                let mut payload = json!({
                    "advertiser_id": adv_id,
                    "campaign_name": request.name,
                    "type": 2,
                    "put_status": put_status,
                });
                if let Some(b_fen) = request.daily_budget_fen {
                    payload["day_budget"] = json!(b_fen.saturating_mul(10));
                }
                ("/v1/campaign/create", payload)
            }
            AdCreationEntityKind::AdGroup => {
                let campaign_id = request.parent_entity_id.clone().ok_or_else(|| {
                    ConnectorError::InvalidRequest {
                        platform: PlatformId::Kuaishou,
                        message: "creating Kuaishou ad_unit requires parent_entity_id (campaign_id)".into(),
                    }
                })?;
                let mut payload = json!({
                    "advertiser_id": adv_id,
                    "campaign_id": campaign_id.parse::<i64>().map(Value::from).unwrap_or_else(|_| json!(campaign_id)),
                    "unit_name": request.name,
                    "put_status": put_status,
                });
                if let Some(b_fen) = request.daily_budget_fen {
                    payload["day_budget"] = json!(b_fen.saturating_mul(10));
                }
                if let Some(bid_fen) = request.bid_fen {
                    payload["bid"] = json!(bid_fen.saturating_mul(10));
                }
                if !request.targeting.is_null() {
                    payload["target"] = request.targeting.clone();
                }
                if let Some(url) = &request.landing_url {
                    payload["url"] = json!(url);
                }
                ("/v2/ad_unit/create", payload)
            }
            AdCreationEntityKind::Creative => {
                let unit_id = request.parent_entity_id.clone().ok_or_else(|| {
                    ConnectorError::InvalidRequest {
                        platform: PlatformId::Kuaishou,
                        message: "creating Kuaishou creative requires parent_entity_id (unit_id)".into(),
                    }
                })?;
                let payload = json!({
                    "advertiser_id": adv_id,
                    "unit_id": unit_id.parse::<i64>().map(Value::from).unwrap_or_else(|_| json!(unit_id)),
                    "creative_name": request.name,
                    "put_status": put_status,
                    "creative_material": request.creative_assets,
                });
                ("/v2/creative/create", payload)
            }
        };
        merge_json_extra(&mut body, &request.platform_extra);

        let risk_warnings = platform_creation_notes(
            PlatformId::Kuaishou,
            Some("Kuaishou creation APIs use `li` (厘: 1 Fen = 10 厘); AdLink automatically converts `daily_budget_fen * 10` and `bid_fen * 10` (whereas report `charge` is in Yuan)."),
        );

        if request.dry_run || matches!(self.mode, ConnectorMode::Mock) {
            let created_id = if request.dry_run {
                None
            } else {
                Some(format!(
                    "ks_new_{}",
                    &Uuid::new_v4().simple().to_string()[..8]
                ))
            };
            return Ok(AdCreationResult {
                dry_run: request.dry_run,
                accepted: true,
                platform: PlatformId::Kuaishou,
                ad_account_id: account.id.clone(),
                entity_kind: request.entity_kind,
                created_entity_id: created_id.clone(),
                initial_status: request.initial_status.clone(),
                summary: format!(
                    "Kuaishou {:?} creation {} ({})",
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
                    platform: PlatformId::Kuaishou,
                    message: e.to_string(),
                })?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(map_http_status(PlatformId::Kuaishou, status, &text, None));
            }
            let val: Value = serde_json::from_str(&text).unwrap_or(json!({"raw": text}));
            if val.get("code").and_then(Value::as_i64).unwrap_or(0) != 0 {
                return Err(ConnectorError::InvalidRequest {
                    platform: PlatformId::Kuaishou,
                    message: format!("Kuaishou create error: {val}"),
                });
            }
            let data = val.get("data").unwrap_or(&val);
            let created_id = json_opt_str(data, "campaign_id")
                .or_else(|| json_opt_str(data, "unit_id"))
                .or_else(|| json_opt_str(data, "creative_id"));
            return Ok(AdCreationResult {
                dry_run: false,
                accepted: true,
                platform: PlatformId::Kuaishou,
                ad_account_id: account.id.clone(),
                entity_kind: request.entity_kind,
                created_entity_id: created_id,
                initial_status: request.initial_status.clone(),
                summary: format!("Kuaishou live {:?} created", request.entity_kind),
                risk_warnings,
                guardrail_notes: vec![],
                platform_payload_preview: body,
                platform_response: val,
            });
        }
        unreachable!()
    }
}

trait EmptyFallback {
    fn if_empty_str(self, f: impl FnOnce() -> String) -> String;
}

impl EmptyFallback for String {
    fn if_empty_str(self, f: impl FnOnce() -> String) -> String {
        if self.trim().is_empty() {
            f()
        } else {
            self
        }
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
