//! Bilibili Sanlian / Commercial Qifei (`B站广告后台三连 / 商业起飞`) MAPI connector.
//!
//! Official partner domain: `https://cm.bilibili.com/takumi/api`
//! Headers: `Authorization: Bearer {access_token}`, `X-Call-Source: {call_source}`

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
    build_http_client, discover_via_official_mcp_bridge, extract_raw_map, json_i64, json_opt_str,
    json_str, json_u64, map_http_status, merge_json_extra, mock_discovered_accounts,
    platform_creation_notes, ConnectorMode,
};
use crate::mock_data::mock_entities_for_account;
use crate::mock_generator::synthesize_chunk_metrics;
use crate::onboarding::build_onboard_guide;

pub struct BilibiliSanlianConnector {
    mode: ConnectorMode,
    client: Client,
}

impl BilibiliSanlianConnector {
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
        "https://cm.bilibili.com/takumi/api"
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
                .unwrap_or("bilibili mapi error");
            return Err(ConnectorError::InvalidRequest {
                platform: PlatformId::BilibiliSanlian,
                message: format!("Bilibili code {code}: {msg}"),
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
            let date_s = {
                let d = json_str(&item, "stat_date");
                if d.is_empty() {
                    json_str(&item, "date")
                } else {
                    d
                }
            };
            let stat_date = NaiveDate::parse_from_str(&date_s, "%Y-%m-%d").map_err(|e| {
                ConnectorError::ParseError {
                    platform: PlatformId::BilibiliSanlian,
                    message: format!("invalid stat_date '{date_s}': {e}"),
                }
            })?;

            let entity_id = match level {
                ReportLevel::Account => account.platform_account_id.clone(),
                ReportLevel::Campaign => json_str(&item, "campaign_id"),
                ReportLevel::AdGroup => json_str(&item, "unit_id"),
                ReportLevel::Creative | ReportLevel::ContentOrder => {
                    let c = json_str(&item, "creative_id");
                    if c.is_empty() {
                        json_str(&item, "bvid")
                    } else {
                        c
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
                        let c = json_str(&item, "campaign_name");
                        if !c.is_empty() {
                            c
                        } else {
                            let e = json_str(&item, "entity_name");
                            if e.is_empty() {
                                account.account_name.clone()
                            } else {
                                e
                            }
                        }
                    }
                }
            };
            let gross_fen = Fen::from_fen(json_i64(&item, "cost"));
            let cash_fen = gross_fen.scale(account.default_cash_ratio);
            let interactions = json_u64(&item, "like_cnt")
                + json_u64(&item, "coin_cnt")
                + json_u64(&item, "fav_cnt")
                + json_u64(&item, "share_cnt");

            rows.push(UnifiedReportRow {
                platform: PlatformId::BilibiliSanlian,
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
                parent_entity_id: json_opt_str(&item, "campaign_id"),
                content_id: json_opt_str(&item, "bvid"),
                time_line,
                gross_cost_fen: gross_fen,
                cash_cost_fen: cash_fen,
                grant_cost_fen: gross_fen - cash_fen,
                impressions: json_u64(&item, "impression"),
                clicks: json_u64(&item, "click"),
                conversions: json_u64(&item, "convert_count"),
                orders: json_u64(&item, "order_count"),
                gmv_fen: Fen::from_fen(json_i64(&item, "gmv_fen")),
                interactions,
                video_plays: json_u64(&item, "play_cnt"),
                effective_plays: json_u64(&item, "effective_play_cnt"),
                new_follows: json_u64(&item, "follow_cnt"),
                raw_fields: extract_raw_map(&item),
                ..Default::default()
            });
        }

        let total_count = data.get("total").and_then(Value::as_u64);
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
        let list: Vec<Value> = items
            .into_iter()
            .map(|(dt, m)| {
                let likes = (m.interactions as f64 * 0.45).round() as u64;
                let coins = (m.interactions as f64 * 0.25).round() as u64;
                let favs = (m.interactions as f64 * 0.20).round() as u64;
                let shares = m.interactions.saturating_sub(likes + coins + favs);
                json!({
                    "stat_date": dt.format("%Y-%m-%d").to_string(),
                    "hour": if m.stat_hour >= 0 { json!(m.stat_hour) } else { Value::Null },
                    "account_id": req.account.platform_account_id,
                    "campaign_id": m.parent_entity_id.clone().unwrap_or_else(|| "bl_camp_01".into()),
                    "creative_id": m.entity_id,
                    "entity_name": m.entity_name,
                    "bvid": m.content_id,
                    "cost": m.gross_cost_fen.as_fen(),
                    "impression": m.impressions,
                    "click": m.clicks,
                    "play_cnt": m.video_plays,
                    "effective_play_cnt": m.effective_plays,
                    "like_cnt": likes,
                    "coin_cnt": coins,
                    "fav_cnt": favs,
                    "share_cnt": shares,
                    "danmaku_cnt": m.interactions / 10,
                    "convert_count": m.conversions,
                    "order_count": m.orders,
                    "gmv_fen": m.gmv_fen.as_fen(),
                    "follow_cnt": m.new_follows,
                })
            })
            .collect();
        json!({ "code": 0, "message": "0", "data": { "list": list, "has_more": has_more, "total": total } })
    }
}

#[async_trait]
impl AdPlatformConnector for BilibiliSanlianConnector {
    fn platform_id(&self) -> PlatformId {
        PlatformId::BilibiliSanlian
    }

    fn mode_label(&self) -> &'static str {
        match self.mode {
            ConnectorMode::Mock => "mock",
            ConnectorMode::Live(_) => "live",
        }
    }

    fn onboard_guide(&self) -> PlatformOnboardGuide {
        build_onboard_guide(PlatformId::BilibiliSanlian, self.mode_label())
    }

    async fn discover_accounts(&self) -> ConnectorResult<Vec<DiscoveredPlatformAccount>> {
        match &self.mode {
            ConnectorMode::Mock => Ok(mock_discovered_accounts(
                PlatformId::BilibiliSanlian,
                AuthLoginStrategy::OfficialMcpBridge,
            )),
            ConnectorMode::Live(creds) => {
                if let Some(bridged) = discover_via_official_mcp_bridge(
                    &self.client,
                    PlatformId::BilibiliSanlian,
                    creds,
                )
                .await
                {
                    return Ok(bridged);
                }
                let url = format!("{}/account/list", self.base_url());
                let mut req_builder = self
                    .client
                    .get(&url)
                    .bearer_auth(&creds.access_token)
                    .header("X-Call-Source", creds.call_source.as_str());
                if let Some(ver) = creds.extra.get("skills_version") {
                    req_builder = req_builder.header("X-Sanlian-Skills-Version", ver.as_str());
                }
                let resp = req_builder.send().await.map_err(|e| ConnectorError::Transient {
                    platform: PlatformId::BilibiliSanlian,
                    message: e.to_string(),
                })?;
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if !status.is_success() {
                    return Err(map_http_status(PlatformId::BilibiliSanlian, status, &text, None));
                }
                let val: Value =
                    serde_json::from_str(&text).map_err(|e| ConnectorError::ParseError {
                        platform: PlatformId::BilibiliSanlian,
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
                        let pid = json_str(&item, "account_id");
                        if pid.is_empty() {
                            return None;
                        }
                        let name = {
                            let n = json_str(&item, "account_name");
                            if n.is_empty() {
                                format!("Bilibili {pid}")
                            } else {
                                n
                            }
                        };
                        Some(DiscoveredPlatformAccount {
                            platform: PlatformId::BilibiliSanlian,
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
                let url = format!("{}/report/query", self.base_url());
                let mut builder = self
                    .client
                    .post(&url)
                    .bearer_auth(&creds.access_token)
                    .header("X-Call-Source", creds.call_source.as_str());
                if let Some(ver) = creds.extra.get("skills_version") {
                    builder = builder.header("X-Sanlian-Skills-Version", ver.as_str());
                }
                let resp = builder
                    .json(&json!({
                        "account_id": req.account.platform_account_id,
                        "start_date": req.start_date.format("%Y-%m-%d").to_string(),
                        "end_date": req.end_date.format("%Y-%m-%d").to_string(),
                        "level": req.level.as_str(),
                        "page": req.page,
                        "page_size": req.page_size,
                    }))
                    .send()
                    .await
                    .map_err(|e| ConnectorError::Transient {
                        platform: PlatformId::BilibiliSanlian,
                        message: e.to_string(),
                    })?;
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if !status.is_success() {
                    return Err(map_http_status(PlatformId::BilibiliSanlian, status, &text, None));
                }
                serde_json::from_str(&text).map_err(|e| ConnectorError::ParseError {
                    platform: PlatformId::BilibiliSanlian,
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
            let gross = Fen::from_yuan_f64(2200.0);
            let cash = gross.scale(account.default_cash_ratio);
            out.push(DailyFundStatement {
                platform: PlatformId::BilibiliSanlian,
                ad_account_id: account.id.clone(),
                platform_account_id: account.platform_account_id.clone(),
                stat_date: cur,
                cash_balance_fen: Fen(64_000_00),
                grant_balance_fen: Fen(8_500_00),
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
                platform: PlatformId::BilibiliSanlian,
                ad_account_id: account.id.clone(),
                affected_entity_ids: request.entity_ids.clone(),
                summary: format!("Bilibili Sanlian {:?} applied", request.action),
                guardrail_notes: vec![],
                platform_response: json!({"code": 0}),
            });
        }

        if let ConnectorMode::Live(creds) = &self.mode {
            let url = format!("{}/campaign/status", self.base_url());
            let op = match &request.action {
                AdMutationAction::Pause => "pause",
                AdMutationAction::Resume => "resume",
                AdMutationAction::UpdateBudget { .. } => "update_budget",
                AdMutationAction::UpdateBid { .. } => "update_bid",
            };
            let resp = self
                .client
                .post(&url)
                .bearer_auth(&creds.access_token)
                .header("X-Call-Source", creds.call_source.as_str())
                .json(&json!({
                    "account_id": account.platform_account_id,
                    "entity_ids": request.entity_ids,
                    "operation": op,
                    "action": request.action,
                }))
                .send()
                .await
                .map_err(|e| ConnectorError::Transient {
                    platform: PlatformId::BilibiliSanlian,
                    message: e.to_string(),
                })?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(map_http_status(PlatformId::BilibiliSanlian, status, &text, None));
            }
            return Ok(AdMutationResult {
                dry_run: false,
                accepted: true,
                platform: PlatformId::BilibiliSanlian,
                ad_account_id: account.id.clone(),
                affected_entity_ids: request.entity_ids.clone(),
                summary: "Bilibili Sanlian live mutation submitted".into(),
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
        let paused = request.initial_status.eq_ignore_ascii_case("PAUSED");
        let path = match request.entity_kind {
            AdCreationEntityKind::Campaign => "/campaign/create",
            AdCreationEntityKind::AdGroup => "/unit/create",
            AdCreationEntityKind::Creative => "/creative/create",
        };
        let mut body = json!({
            "account_id": account.platform_account_id,
            "name": request.name,
            "parent_entity_id": request.parent_entity_id,
            "daily_budget_fen": request.daily_budget_fen,
            "bid_fen": request.bid_fen,
            "status": if paused { "PAUSED" } else { "ENABLED" },
            "creative_assets": request.creative_assets,
        });
        merge_json_extra(&mut body, &request.platform_extra);

        let risk_warnings = platform_creation_notes(
            PlatformId::BilibiliSanlian,
            Some("Bilibili Sanlian/Qifei creation follows partner NDA endpoints (configurable via base_url_override)."),
        );
        if request.dry_run || matches!(self.mode, ConnectorMode::Mock) {
            let created_id = if request.dry_run {
                None
            } else {
                Some(format!(
                    "bl_new_{}",
                    &Uuid::new_v4().simple().to_string()[..8]
                ))
            };
            return Ok(AdCreationResult {
                dry_run: request.dry_run,
                accepted: true,
                platform: PlatformId::BilibiliSanlian,
                ad_account_id: account.id.clone(),
                entity_kind: request.entity_kind,
                created_entity_id: created_id.clone(),
                initial_status: request.initial_status.clone(),
                summary: format!(
                    "Bilibili Sanlian {:?} creation {} ({})",
                    request.entity_kind,
                    if request.dry_run { "previewed" } else { "created in mock" },
                    self.mode_label()
                ),
                risk_warnings,
                guardrail_notes: vec![format!("Partner endpoint: POST {}{}", self.base_url(), path)],
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
            let mut builder = self
                .client
                .post(&url)
                .bearer_auth(&creds.access_token)
                .header("X-Call-Source", creds.call_source.as_str());
            if let Some(ver) = creds.extra.get("skills_version") {
                builder = builder.header("X-Sanlian-Skills-Version", ver.as_str());
            }
            let resp = builder
                .json(&body)
                .send()
                .await
                .map_err(|e| ConnectorError::Transient {
                    platform: PlatformId::BilibiliSanlian,
                    message: e.to_string(),
                })?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(map_http_status(PlatformId::BilibiliSanlian, status, &text, None));
            }
            let val: Value = serde_json::from_str(&text).unwrap_or(json!({"raw": text}));
            if val.get("code").and_then(Value::as_i64).unwrap_or(0) != 0 {
                return Err(ConnectorError::InvalidRequest {
                    platform: PlatformId::BilibiliSanlian,
                    message: format!("Bilibili create error: {val}"),
                });
            }
            let data = val.get("data").unwrap_or(&val);
            let created_id = json_opt_str(data, "id")
                .or_else(|| json_opt_str(data, "campaign_id"))
                .or_else(|| json_opt_str(data, "unit_id"))
                .or_else(|| json_opt_str(data, "creative_id"));
            return Ok(AdCreationResult {
                dry_run: false,
                accepted: true,
                platform: PlatformId::BilibiliSanlian,
                ad_account_id: account.id.clone(),
                entity_kind: request.entity_kind,
                created_entity_id: created_id,
                initial_status: request.initial_status.clone(),
                summary: format!("Bilibili Sanlian live {:?} created", request.entity_kind),
                risk_warnings,
                guardrail_notes: vec![],
                platform_payload_preview: body,
                platform_response: val,
            });
        }
        unreachable!()
    }
}

pub struct BilibiliSanlianPlugin {
    connector: Arc<BilibiliSanlianConnector>,
}

impl BilibiliSanlianPlugin {
    pub fn new(mode: ConnectorMode) -> Self {
        Self {
            connector: Arc::new(BilibiliSanlianConnector::new(mode)),
        }
    }
}

#[async_trait]
impl Plugin for BilibiliSanlianPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "connector.bilibili_sanlian".into(),
            name: "Bilibili Sanlian / Qifei Connector".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Official commercial MAPI connector for B站广告后台三连 & 商业起飞".into(),
            category: PluginCategory::Connector,
            provides: vec!["connector:bilibili_sanlian".into()],
            requires: vec!["storage".into()],
            config_schema: json!({"type": "object"}),
        }
    }

    async fn mount(&self, ctx: &mut PluginContext) -> KernelResult<()> {
        let svc: Arc<dyn AdPlatformConnector> =
            Arc::clone(&self.connector) as Arc<dyn AdPlatformConnector>;
        ctx.provide_named_service(PlatformId::BilibiliSanlian.as_str(), svc);
        Ok(())
    }
}
