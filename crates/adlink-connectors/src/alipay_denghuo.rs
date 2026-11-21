//! Alipay Denghuo (`支付宝灯火`) OpenAPI connector with pure-Rust RSA2 (SHA256WithRSA) signing.
//!
//! Official methods (`https://openapi.alipay.com/gateway.do`):
//! - `alipay.data.dataservice.ad.reportdata.query` (`biz_token`, `alipay_pid`, `query_type`, `ad_level`, `yyyyMMdd`)
//! - `alipay.data.dataservice.ad.consumehistory.query`

use std::collections::BTreeMap;
use std::sync::Arc;

use adlink_domain::{
    AccountType, AdAccount, AdEntity, AdMutationRequest, AdMutationResult, AuthLoginStrategy,
    DailyFundStatement, DiscoveredPlatformAccount, Fen, PlatformId, ReportGranularity,
    ReportLevel, TimeLine, UnifiedReportRow,
};
use adlink_engine::{
    AdPlatformConnector, ConnectorError, ConnectorResult, PlatformOnboardGuide, ReportFetchPage,
    ReportFetchRequest,
};
use adlink_kernel::{KernelResult, Plugin, PluginCategory, PluginContext, PluginManifest};
use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chrono::{FixedOffset, NaiveDate, Utc};
use reqwest::Client;
use ring::rand::SystemRandom;
use ring::signature::{RsaKeyPair, RSA_PKCS1_SHA256};
use serde_json::{json, Value};

use crate::http_helpers::{
    build_http_client, discover_via_official_mcp_bridge, extract_raw_map, json_i64, json_opt_str,
    json_str, json_u64, map_http_status, mock_discovered_accounts, ConnectorMode,
};
use crate::mock_data::mock_entities_for_account;
use crate::mock_generator::synthesize_chunk_metrics;
use crate::onboarding::build_onboard_guide;

/// Builds the official Alipay OpenAPI canonical signing string (`k1=v1&k2=v2...` sorted ASCII).
pub fn build_alipay_sign_content(params: &BTreeMap<String, String>) -> String {
    params
        .iter()
        .filter(|(k, v)| k.as_str() != "sign" && !v.is_empty())
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// Decodes PEM or raw base64 PKCS#8 RSA private key and signs `content` using RSA2 (SHA256WithRSA).
pub fn sign_alipay_rsa2(private_key_pem: &str, content: &str) -> Result<String, String> {
    let der_b64: String = private_key_pem
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("-----") && !line.is_empty())
        .collect();
    let der = BASE64
        .decode(der_b64.as_bytes())
        .map_err(|e| format!("base64 decode rsa2 private key failed: {e}"))?;
    let key_pair = RsaKeyPair::from_pkcs8(&der)
        .map_err(|e| format!("invalid PKCS#8 RSA2 private key: {e}"))?;
    let rng = SystemRandom::new();
    let mut sig = vec![0u8; key_pair.public().modulus_len()];
    key_pair
        .sign(&RSA_PKCS1_SHA256, &rng, content.as_bytes(), &mut sig)
        .map_err(|e| format!("RSA2 sign failed: {e}"))?;
    Ok(BASE64.encode(&sig))
}

pub struct AlipayDenghuoConnector {
    mode: ConnectorMode,
    client: Client,
}

impl AlipayDenghuoConnector {
    pub fn new(mode: ConnectorMode) -> Self {
        Self {
            mode,
            client: build_http_client(),
        }
    }

    fn gateway_url(&self) -> &str {
        if let ConnectorMode::Live(creds) = &self.mode {
            if let Some(u) = &creds.base_url_override {
                return u.as_str();
            }
        }
        "https://openapi.alipay.com/gateway.do"
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
        let resp = payload
            .get("alipay_data_dataservice_ad_reportdata_query_response")
            .unwrap_or(payload);
        let code = json_str(resp, "code");
        if !code.is_empty() && code != "10000" {
            let sub_msg = json_str(resp, "sub_msg");
            let msg = json_str(resp, "msg");
            return Err(ConnectorError::InvalidRequest {
                platform: PlatformId::AlipayDenghuo,
                message: format!("Alipay code {code}: {msg} ({sub_msg})"),
            });
        }

        let list = resp
            .get("data_list")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let mut rows = Vec::with_capacity(list.len());
        for item in list {
            let biz_date = json_str(&item, "biz_date");
            let stat_date = NaiveDate::parse_from_str(&biz_date, "%Y%m%d")
                .or_else(|_| NaiveDate::parse_from_str(&biz_date, "%Y-%m-%d"))
                .map_err(|e| ConnectorError::ParseError {
                    platform: PlatformId::AlipayDenghuo,
                    message: format!("invalid biz_date '{biz_date}': {e}"),
                })?;

            // Denghuo `cost` is reported in Fen (分).
            let gross_fen = Fen::from_fen(json_i64(&item, "cost"));
            let cash_fen = gross_fen.scale(account.default_cash_ratio);

            let mut conv_total = json_u64(&item, "conv_result");
            let mut conv_list_sum = 0u64;
            let mut leads = 0u64;
            let mut orders = 0u64;
            let mut gmv_fen = Fen::from_fen(json_i64(&item, "gmv_fen"));
            let mut conv_gmv_fen = Fen::ZERO;
            // Official `alipay.data.dataservice.ad.reportdata.query` returns `conversion_data_list`
            // (`conv_data_list` retained only as backward-compatible alias).
            if let Some(conv_list) = item
                .get("conversion_data_list")
                .or_else(|| item.get("conv_data_list"))
                .and_then(Value::as_array)
            {
                for c in conv_list {
                    let metric_code = json_str(c, "conversion_type");
                    let metric_name = json_str(c, "conversion_type_name");
                    let val = json_u64(c, "conversion_result");
                    conv_list_sum += val;
                    if metric_code.contains("FORM")
                        || metric_code.contains("LEAD")
                        || metric_name.contains("表单")
                        || metric_name.contains("线索")
                    {
                        leads += val;
                    }
                    if metric_code.contains("ORDER")
                        || metric_code.contains("PAY")
                        || metric_name.contains("支付")
                        || metric_name.contains("下单")
                        || metric_name.contains("成交")
                    {
                        orders += val;
                        let amt = json_i64(c, "conversion_amount_fen");
                        if amt > 0 {
                            conv_gmv_fen += Fen(amt);
                        }
                    }
                }
            }
            if conv_total == 0 && conv_list_sum > 0 {
                conv_total = conv_list_sum;
            }
            // Prevent 2x double counting when both top-level `gmv_fen` and `conversion_data_list` carry order GMV.
            if gmv_fen == Fen::ZERO && conv_gmv_fen > Fen::ZERO {
                gmv_fen = conv_gmv_fen;
            }

            let entity_id = {
                let id = json_str(&item, "data_id");
                if id.is_empty() {
                    account.platform_account_id.clone()
                } else {
                    id
                }
            };
            let entity_name = {
                let n = json_str(&item, "data_name");
                if n.is_empty() {
                    account.account_name.clone()
                } else {
                    n
                }
            };

            rows.push(UnifiedReportRow {
                platform: PlatformId::AlipayDenghuo,
                ad_account_id: account.id.clone(),
                platform_account_id: account.platform_account_id.clone(),
                stat_date,
                stat_hour: item.get("biz_hour").and_then(Value::as_i64).map(|h| h as i32).unwrap_or(-1),
                granularity,
                level,
                entity_id,
                entity_name,
                content_id: json_opt_str(&item, "material_id"),
                time_line,
                gross_cost_fen: gross_fen,
                cash_cost_fen: cash_fen,
                grant_cost_fen: gross_fen - cash_fen,
                impressions: json_u64(&item, "impression"),
                clicks: json_u64(&item, "click"),
                conversions: conv_total,
                leads,
                orders,
                gmv_fen,
                raw_fields: extract_raw_map(&item),
                ..Default::default()
            });
        }

        let total = resp.get("total").and_then(Value::as_u64);
        let fetched_through = u64::from(page.max(1)).saturating_mul(u64::from(page_size.max(1)));
        let has_more = total.is_some_and(|t| fetched_through < t && !rows.is_empty());
        Ok(ReportFetchPage {
            rows,
            has_more,
            total_count: total,
            next_cursor: None,
        })
    }

    fn build_mock_payload(req: &ReportFetchRequest) -> Value {
        let (items, _, total) = synthesize_chunk_metrics(
            &req.account,
            req.start_date,
            req.end_date,
            req.granularity,
            req.level,
            req.page,
            req.page_size,
        );
        let data_list: Vec<Value> = items
            .into_iter()
            .map(|(dt, m)| {
                json!({
                    "biz_date": dt.format("%Y%m%d").to_string(),
                    "biz_hour": if m.stat_hour >= 0 { json!(m.stat_hour) } else { Value::Null },
                    "data_id": m.entity_id,
                    "data_name": m.entity_name,
                    "material_id": m.content_id,
                    "cost": m.gross_cost_fen.as_fen(),
                    "cost_format": format!("{:.2}", m.gross_cost_fen.to_yuan_f64()),
                    "impression": m.impressions,
                    "click": m.clicks,
                    "conv_result": m.conversions,
                    "gmv_fen": m.gmv_fen.as_fen(),
                    "conversion_data_list": [
                        {
                            "conversion_type": "FORM_SUBMIT",
                            "conversion_type_name": "表单提交线索",
                            "conversion_result": m.leads
                        },
                        {
                            "conversion_type": "MINIAPP_PAY_ORDER",
                            "conversion_type_name": "小程序支付成交",
                            "conversion_result": m.orders,
                            "conversion_amount_fen": m.gmv_fen.as_fen()
                        }
                    ]
                })
            })
            .collect();

        json!({
            "alipay_data_dataservice_ad_reportdata_query_response": {
                "code": "10000",
                "msg": "Success",
                "total": total,
                "data_list": data_list
            }
        })
    }
}

#[async_trait]
impl AdPlatformConnector for AlipayDenghuoConnector {
    fn platform_id(&self) -> PlatformId {
        PlatformId::AlipayDenghuo
    }

    fn mode_label(&self) -> &'static str {
        match self.mode {
            ConnectorMode::Mock => "mock",
            ConnectorMode::Live(_) => "live",
        }
    }

    fn onboard_guide(&self) -> PlatformOnboardGuide {
        build_onboard_guide(PlatformId::AlipayDenghuo, self.mode_label())
    }

    async fn discover_accounts(&self) -> ConnectorResult<Vec<DiscoveredPlatformAccount>> {
        match &self.mode {
            ConnectorMode::Mock => Ok(mock_discovered_accounts(
                PlatformId::AlipayDenghuo,
                AuthLoginStrategy::OfficialMcpBridge,
            )),
            ConnectorMode::Live(creds) => {
                if let Some(bridged) = discover_via_official_mcp_bridge(
                    &self.client,
                    PlatformId::AlipayDenghuo,
                    creds,
                )
                .await
                {
                    return Ok(bridged);
                }
                if creds.alipay_pid.trim().is_empty() {
                    return Ok(Vec::new());
                }
                Ok(vec![DiscoveredPlatformAccount {
                    platform: PlatformId::AlipayDenghuo,
                    platform_account_id: creds.alipay_pid.clone(),
                    account_name: format!("支付宝灯火主体 ({})", creds.alipay_pid),
                    account_type: AccountType::BrandDirect,
                    status: "active".into(),
                    parent_agent_id: None,
                    auth_strategy: AuthLoginStrategy::DirectApiAccountDiscovery,
                    already_registered_ad_account_id: None,
                    current_person_ids: Vec::new(),
                    current_project_ids: Vec::new(),
                    raw_extra: json!({ "app_id": creds.app_id }),
                }])
            }
        }
    }

    async fn fetch_reports_page(&self, req: &ReportFetchRequest) -> ConnectorResult<ReportFetchPage> {
        let payload = match &self.mode {
            ConnectorMode::Mock => Self::build_mock_payload(req),
            ConnectorMode::Live(creds) => {
                let ad_level = match req.level {
                    ReportLevel::Account => "PRINCIPAL",
                    ReportLevel::Campaign => "PLAN",
                    ReportLevel::AdGroup => "GROUP",
                    ReportLevel::Creative => "CREATIVE",
                    ReportLevel::ContentOrder => "ORDER",
                };
                let query_type = if req.granularity == ReportGranularity::Hourly {
                    "HOUR"
                } else {
                    "DETAIL"
                };
                // Official Alipay Denghuo docs note PRINCIPAL/MARKET_TARGET do not return conversion_data_list.
                let mut biz_obj = json!({
                    "biz_token": creds.biz_token,
                    "alipay_pid": if creds.alipay_pid.is_empty() {
                        &req.account.platform_account_id
                    } else {
                        &creds.alipay_pid
                    },
                    "query_type": query_type,
                    "ad_level": ad_level,
                    "start_date": req.start_date.format("%Y%m%d").to_string(),
                    "end_date": req.end_date.format("%Y%m%d").to_string(),
                    "conv_time_join_rule": match req.time_line {
                        TimeLine::ReportingTime => "CONVERT_TIME",
                        TimeLine::RequestTime => "CLICK_TIME",
                    },
                    "current": req.page,
                    "page_size": req.page_size,
                });
                if ad_level != "PRINCIPAL" {
                    biz_obj["show_conv_data"] = json!("Y");
                    if let Some(codes) = creds.extra.get("conv_code_list") {
                        let parsed: Vec<&str> = codes
                            .split(',')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .collect();
                        if !parsed.is_empty() {
                            biz_obj["conv_code_list"] = json!(parsed);
                        }
                    }
                }
                let biz_content = biz_obj.to_string();

                let cst = FixedOffset::east_opt(8 * 3600).unwrap();
                let ts = Utc::now()
                    .with_timezone(&cst)
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string();

                let mut params = BTreeMap::new();
                params.insert("app_id".into(), creds.app_id.clone());
                params.insert(
                    "method".into(),
                    "alipay.data.dataservice.ad.reportdata.query".into(),
                );
                params.insert("charset".into(), "utf-8".into());
                params.insert("sign_type".into(), "RSA2".into());
                params.insert("timestamp".into(), ts);
                params.insert("version".into(), "1.0".into());
                params.insert("biz_content".into(), biz_content);

                let sign_content = build_alipay_sign_content(&params);
                let sign = sign_alipay_rsa2(&creds.rsa2_private_key_pem, &sign_content).map_err(
                    |e| ConnectorError::AuthError {
                        platform: PlatformId::AlipayDenghuo,
                        message: e,
                    },
                )?;
                params.insert("sign".into(), sign);

                let resp = self
                    .client
                    .post(self.gateway_url())
                    .form(&params)
                    .send()
                    .await
                    .map_err(|e| ConnectorError::Transient {
                        platform: PlatformId::AlipayDenghuo,
                        message: e.to_string(),
                    })?;
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if !status.is_success() {
                    return Err(map_http_status(PlatformId::AlipayDenghuo, status, &text, None));
                }
                serde_json::from_str(&text).map_err(|e| ConnectorError::ParseError {
                    platform: PlatformId::AlipayDenghuo,
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
            let gross = Fen::from_yuan_f64(2400.0);
            let cash = gross.scale(account.default_cash_ratio);
            out.push(DailyFundStatement {
                platform: PlatformId::AlipayDenghuo,
                ad_account_id: account.id.clone(),
                platform_account_id: account.platform_account_id.clone(),
                stat_date: cur,
                cash_balance_fen: Fen(72_000_00),
                grant_balance_fen: Fen(6_500_00),
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
                platform: PlatformId::AlipayDenghuo,
                ad_account_id: account.id.clone(),
                affected_entity_ids: request.entity_ids.clone(),
                summary: format!(
                    "Alipay Denghuo {:?} simulated ({})",
                    request.action,
                    self.mode_label()
                ),
                guardrail_notes: vec![
                    "Note: Alipay Denghuo public OpenAPI does not expose self-serve plan/group status or budget mutation endpoints; live write requires adpub.alipay.com portal or whitelisted ISV API.".into(),
                ],
                platform_response: json!({"code": "10000", "mock": matches!(self.mode, ConnectorMode::Mock)}),
            });
        }
        Err(ConnectorError::InvalidRequest {
            platform: PlatformId::AlipayDenghuo,
            message: "Alipay Denghuo OpenAPI does not publish self-serve campaign status/budget/bid mutation endpoints".into(),
        })
    }
}

pub struct AlipayDenghuoPlugin {
    connector: Arc<AlipayDenghuoConnector>,
}

impl AlipayDenghuoPlugin {
    pub fn new(mode: ConnectorMode) -> Self {
        Self {
            connector: Arc::new(AlipayDenghuoConnector::new(mode)),
        }
    }
}

#[async_trait]
impl Plugin for AlipayDenghuoPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "connector.alipay_denghuo".into(),
            name: "Alipay Denghuo Connector".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Official RSA2-signed OpenAPI connector for 支付宝灯火广告开放平台".into(),
            category: PluginCategory::Connector,
            provides: vec!["connector:alipay_denghuo".into()],
            requires: vec!["storage".into()],
            config_schema: json!({"type": "object"}),
        }
    }

    async fn mount(&self, ctx: &mut PluginContext) -> KernelResult<()> {
        let svc: Arc<dyn AdPlatformConnector> =
            Arc::clone(&self.connector) as Arc<dyn AdPlatformConnector>;
        ctx.provide_named_service(PlatformId::AlipayDenghuo.as_str(), svc);
        Ok(())
    }
}
