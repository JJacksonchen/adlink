//! Apple Search Ads (Campaign Management API v5 + pure-Rust ES256 client_secret JWT signing).
//!
//! Official endpoints (`https://api.searchads.apple.com/api/v5`):
//! - Headers: `Authorization: Bearer {access_token}`, `X-AP-Context: orgId={org_id}`
//! - `POST /reports/campaigns` (`localSpend { amount, currency }`, `impressions`, `taps`, `tapInstalls`)
//! - `GET /acls`
//! - Opt-in creation: `POST /campaigns`, `POST /campaigns/{id}/adgroups`, `POST /campaigns/{id}/adgroups/{id}/ads`

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
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{NaiveDate, Utc};
use reqwest::Client;
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::http_helpers::{
    base_fen_to_currency_major, build_http_client, discover_via_official_mcp_bridge,
    extract_raw_map, json_opt_str, json_str, json_u64, map_http_status, merge_json_extra,
    mock_discovered_accounts, platform_creation_notes, resolve_account_fx,
    stamp_currency_raw_fields, ConnectorMode,
};
use crate::mock_data::mock_entities_for_account;
use crate::mock_generator::synthesize_chunk_metrics;
use crate::onboarding::build_onboard_guide;

/// Generates a signed ES256 (ECDSA P-256 SHA-256) JWT `client_secret` required by
/// Apple Search Ads OAuth (`https://appleid.apple.com/auth/oauth2/token`).
pub fn sign_apple_es256_client_secret(
    client_id: &str,
    team_id: &str,
    key_id: &str,
    pkcs8_pem: &str,
) -> Result<String, String> {
    let der_b64: String = pkcs8_pem
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("-----") && !line.is_empty())
        .collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(der_b64.as_bytes())
        .map_err(|e| format!("base64 decode ES256 private key failed: {e}"))?;
    let rng = SystemRandom::new();
    let key_pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der, &rng)
        .map_err(|e| format!("invalid PKCS#8 ES256 private key: {e}"))?;

    let now = Utc::now().timestamp();
    let header_json = json!({
        "alg": "ES256",
        "kid": key_id
    });
    let claims_json = json!({
        "sub": client_id,
        "aud": "https://appleid.apple.com",
        "iat": now,
        "exp": now + 86400 * 180,
        "iss": team_id
    });
    let header_b64 = URL_SAFE_NO_PAD.encode(header_json.to_string().as_bytes());
    let claims_b64 = URL_SAFE_NO_PAD.encode(claims_json.to_string().as_bytes());
    let signing_input = format!("{header_b64}.{claims_b64}");

    let sig = key_pair
        .sign(&rng, signing_input.as_bytes())
        .map_err(|e| format!("ES256 JWT sign failed: {e}"))?;
    let sig_b64 = URL_SAFE_NO_PAD.encode(sig.as_ref());
    Ok(format!("{signing_input}.{sig_b64}"))
}

pub struct AppleSearchAdsConnector {
    mode: ConnectorMode,
    client: Client,
}

impl AppleSearchAdsConnector {
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
        "https://api.searchads.apple.com/api/v5"
    }

    /// Parses official Apple Search Ads API v5 `reportingDataResponse` JSON payload.
    pub fn parse_official_reports_response(
        payload: &Value,
        account: &AdAccount,
        granularity: ReportGranularity,
        level: ReportLevel,
        time_line: TimeLine,
    ) -> ConnectorResult<ReportFetchPage> {
        if let Some(err) = payload.get("error").filter(|e| !e.is_null()) {
            let msg = err.to_string();
            return Err(ConnectorError::InvalidRequest {
                platform: PlatformId::AppleSearchAds,
                message: format!("ASA v5 error: {msg}"),
            });
        }

        let report_rows = payload
            .get("data")
            .and_then(|d| d.get("reportingDataResponse"))
            .and_then(|r| r.get("row"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let mut rows = Vec::new();
        for entry in report_rows {
            let meta = entry.get("metadata").unwrap_or(&Value::Null);
            let campaign_id = json_str(meta, "campaignId");
            let campaign_name = json_str(meta, "campaignName");
            let adgroup_id = json_str(meta, "adGroupId");
            let adgroup_name = json_str(meta, "adGroupName");

            let entity_id = match level {
                ReportLevel::Account => account.platform_account_id.clone(),
                ReportLevel::Campaign => campaign_id.clone(),
                ReportLevel::AdGroup => adgroup_id.clone(),
                ReportLevel::Creative | ReportLevel::ContentOrder => {
                    json_opt_str(meta, "adId")
                        .or_else(|| json_opt_str(meta, "keywordId"))
                        .unwrap_or_else(|| campaign_id.clone())
                }
            };
            let entity_name = match level {
                ReportLevel::Campaign => {
                    if campaign_name.is_empty() {
                        account.account_name.clone()
                    } else {
                        campaign_name
                    }
                }
                ReportLevel::AdGroup => {
                    if adgroup_name.is_empty() {
                        campaign_name
                    } else {
                        adgroup_name
                    }
                }
                _ => {
                    let ad_name = json_str(meta, "adName");
                    if !ad_name.is_empty() {
                        ad_name
                    } else if !campaign_name.is_empty() {
                        campaign_name
                    } else {
                        account.account_name.clone()
                    }
                }
            };

            let gran_list = entry
                .get("granularity")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_else(|| {
                    entry
                        .get("total")
                        .cloned()
                        .map(|t| vec![t])
                        .unwrap_or_default()
                });

            for g in gran_list {
                let date_str = json_str(&g, "date");
                let date_only = date_str.get(..10).unwrap_or(date_str.as_str());
                let stat_date = NaiveDate::parse_from_str(date_only, "%Y-%m-%d").map_err(|e| {
                    ConnectorError::ParseError {
                        platform: PlatformId::AppleSearchAds,
                        message: format!("invalid granularity.date '{date_str}': {e}"),
                    }
                })?;

                let local_spend = g.get("localSpend").unwrap_or(&Value::Null);
                let spend_major = json_str(local_spend, "amount")
                    .parse::<f64>()
                    .unwrap_or_else(|_| {
                        local_spend
                            .get("amount")
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0)
                    });
                let (currency, fx_rate) = resolve_account_fx(
                    account,
                    local_spend.get("currency").and_then(Value::as_str),
                );
                let gross_fen = Fen::from_yuan_f64(spend_major * fx_rate);
                let cash_fen = gross_fen.scale(account.default_cash_ratio);

                let tap_installs = json_u64(&g, "tapInstalls")
                    .max(json_u64(&g, "totalInstalls"))
                    .max(json_u64(&g, "installs"));

                let mut raw = extract_raw_map(&g);
                if let Some(m) = meta.as_object() {
                    for (k, v) in m {
                        raw.insert(k.clone(), v.clone());
                    }
                }
                stamp_currency_raw_fields(&mut raw, &currency, fx_rate, spend_major);

                rows.push(UnifiedReportRow {
                    platform: PlatformId::AppleSearchAds,
                    ad_account_id: account.id.clone(),
                    platform_account_id: account.platform_account_id.clone(),
                    stat_date,
                    stat_hour: -1,
                    granularity,
                    level,
                    entity_id: if entity_id.is_empty() {
                        account.platform_account_id.clone()
                    } else {
                        entity_id.clone()
                    },
                    entity_name: entity_name.clone(),
                    parent_entity_id: (!campaign_id.is_empty()).then(|| campaign_id.clone()),
                    content_id: json_opt_str(meta, "adamId"),
                    time_line,
                    gross_cost_fen: gross_fen,
                    cash_cost_fen: cash_fen,
                    grant_cost_fen: gross_fen - cash_fen,
                    impressions: json_u64(&g, "impressions"),
                    clicks: json_u64(&g, "taps"),
                    conversions: tap_installs,
                    orders: tap_installs,
                    gmv_fen: Fen::ZERO,
                    raw_fields: raw,
                    ..Default::default()
                });
            }
        }

        let pagination = payload
            .get("pagination")
            .unwrap_or(&Value::Null);
        let total = pagination.get("totalResults").and_then(Value::as_u64);
        Ok(ReportFetchPage {
            rows,
            has_more: false,
            total_count: total,
            next_cursor: None,
        })
    }

    fn build_mock_payload(req: &ReportFetchRequest) -> Value {
        let (currency, fx_rate) = resolve_account_fx(&req.account, None);
        let (items, _, total) = synthesize_chunk_metrics(
            &req.account,
            req.start_date,
            req.end_date,
            ReportGranularity::Daily,
            req.level,
            1,
            req.page_size.max(500),
        );
        let rows: Vec<Value> = items
            .into_iter()
            .map(|(dt, m)| {
                let spend_major =
                    ((m.gross_cost_fen.to_yuan_f64() / fx_rate) * 100.0).round() / 100.0;
                json!({
                    "metadata": {
                        "campaignId": m.parent_entity_id.clone().unwrap_or_else(|| "asa_camp_01".into()),
                        "campaignName": m.entity_name,
                        "adGroupId": m.entity_id,
                        "adGroupName": m.entity_name,
                        "adamId": m.content_id.unwrap_or_else(|| "6449102831".into()),
                    },
                    "granularity": [{
                        "date": dt.format("%Y-%m-%d").to_string(),
                        "localSpend": {
                            "amount": format!("{spend_major:.2}"),
                            "currency": currency,
                        },
                        "impressions": m.impressions,
                        "taps": m.clicks,
                        "tapInstalls": m.conversions,
                        "newDownloads": (m.conversions * 3) / 4,
                        "redownloads": m.conversions / 4,
                    }]
                })
            })
            .collect();
        json!({
            "data": {
                "reportingDataResponse": {
                    "row": rows
                }
            },
            "pagination": {
                "totalResults": total,
                "startIndex": 0,
                "itemsPerPage": req.page_size
            }
        })
    }
}

#[async_trait]
impl AdPlatformConnector for AppleSearchAdsConnector {
    fn platform_id(&self) -> PlatformId {
        PlatformId::AppleSearchAds
    }

    fn mode_label(&self) -> &'static str {
        match self.mode {
            ConnectorMode::Mock => "mock",
            ConnectorMode::Live(_) => "live",
        }
    }

    fn onboard_guide(&self) -> PlatformOnboardGuide {
        build_onboard_guide(PlatformId::AppleSearchAds, self.mode_label())
    }

    async fn discover_accounts(&self) -> ConnectorResult<Vec<DiscoveredPlatformAccount>> {
        match &self.mode {
            ConnectorMode::Mock => Ok(mock_discovered_accounts(
                PlatformId::AppleSearchAds,
                AuthLoginStrategy::OfficialMcpBridge,
            )),
            ConnectorMode::Live(creds) => {
                if let Some(bridged) = discover_via_official_mcp_bridge(
                    &self.client,
                    PlatformId::AppleSearchAds,
                    creds,
                )
                .await
                {
                    return Ok(bridged);
                }
                let url = format!("{}/acls", self.base_url());
                let resp = self
                    .client
                    .get(&url)
                    .bearer_auth(&creds.access_token)
                    .send()
                    .await
                    .map_err(|e| ConnectorError::Transient {
                        platform: PlatformId::AppleSearchAds,
                        message: e.to_string(),
                    })?;
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if !status.is_success() {
                    return Err(map_http_status(PlatformId::AppleSearchAds, status, &text, None));
                }
                let val: Value =
                    serde_json::from_str(&text).map_err(|e| ConnectorError::ParseError {
                        platform: PlatformId::AppleSearchAds,
                        message: e.to_string(),
                    })?;
                let list = val
                    .get("data")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                Ok(list
                    .into_iter()
                    .filter_map(|item| {
                        let oid = json_str(&item, "orgId");
                        if oid.is_empty() {
                            return None;
                        }
                        let name = {
                            let n = json_str(&item, "orgName");
                            if n.is_empty() {
                                format!("Apple Search Ads Org {oid}")
                            } else {
                                n
                            }
                        };
                        Some(DiscoveredPlatformAccount {
                            platform: PlatformId::AppleSearchAds,
                            platform_account_id: oid,
                            account_name: name,
                            account_type: AccountType::BrandDirect,
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
                let path = match req.level {
                    ReportLevel::AdGroup => "/reports/campaigns/adgroups",
                    _ => "/reports/campaigns",
                };
                let url = format!("{}{}", self.base_url(), path);
                let org_ctx = if creds.org_id.is_empty() {
                    format!("orgId={}", req.account.platform_account_id)
                } else {
                    format!("orgId={}", creds.org_id)
                };
                let resp = self
                    .client
                    .post(&url)
                    .bearer_auth(&creds.access_token)
                    .header("X-AP-Context", org_ctx)
                    .json(&json!({
                        "startTime": req.start_date.format("%Y-%m-%d").to_string(),
                        "endTime": req.end_date.format("%Y-%m-%d").to_string(),
                        "granularity": "DAILY",
                        "returnRowTotals": true,
                        "returnGrandTotals": false,
                        "pagination": {
                            "offset": req.page.saturating_sub(1) * req.page_size,
                            "limit": req.page_size.min(1000)
                        }
                    }))
                    .send()
                    .await
                    .map_err(|e| ConnectorError::Transient {
                        platform: PlatformId::AppleSearchAds,
                        message: e.to_string(),
                    })?;
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if !status.is_success() {
                    return Err(map_http_status(PlatformId::AppleSearchAds, status, &text, None));
                }
                serde_json::from_str(&text).map_err(|e| ConnectorError::ParseError {
                    platform: PlatformId::AppleSearchAds,
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
            let gross = Fen::from_yuan_f64(3100.0);
            let cash = gross.scale(account.default_cash_ratio);
            out.push(DailyFundStatement {
                platform: PlatformId::AppleSearchAds,
                ad_account_id: account.id.clone(),
                platform_account_id: account.platform_account_id.clone(),
                stat_date: cur,
                cash_balance_fen: Fen(180_000_00),
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
                platform: PlatformId::AppleSearchAds,
                ad_account_id: account.id.clone(),
                affected_entity_ids: request.entity_ids.clone(),
                summary: format!("Apple Search Ads v5 {:?} applied ({})", request.action, self.mode_label()),
                guardrail_notes: vec![],
                platform_response: json!({"status": "ok"}),
            });
        }
        if let ConnectorMode::Live(creds) = &self.mode {
            let cid = request.entity_ids.first().cloned().unwrap_or_default();
            let (currency, fx_rate) = resolve_account_fx(account, None);
            let mut body = json!({});
            match &request.action {
                AdMutationAction::Pause => body["status"] = json!("PAUSED"),
                AdMutationAction::Resume => body["status"] = json!("ENABLED"),
                AdMutationAction::UpdateBudget { daily_budget_fen } => {
                    body["dailyBudgetAmount"] = json!({
                        "amount": format!("{:.2}", base_fen_to_currency_major(*daily_budget_fen, fx_rate)),
                        "currency": currency
                    });
                }
                AdMutationAction::UpdateBid { .. } => {
                    return Err(ConnectorError::InvalidRequest {
                        platform: PlatformId::AppleSearchAds,
                        message: "campaign PUT supports Pause/Resume/UpdateBudget; keyword CPT bids update via /adgroups/{id}/targetingkeywords/bulk".into(),
                    })
                }
            }
            let url = format!("{}/campaigns/{cid}", self.base_url());
            let org_ctx = if creds.org_id.is_empty() {
                format!("orgId={}", account.platform_account_id)
            } else {
                format!("orgId={}", creds.org_id)
            };
            let resp = self
                .client
                .put(&url)
                .bearer_auth(&creds.access_token)
                .header("X-AP-Context", org_ctx)
                .json(&body)
                .send()
                .await
                .map_err(|e| ConnectorError::Transient {
                    platform: PlatformId::AppleSearchAds,
                    message: e.to_string(),
                })?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(map_http_status(PlatformId::AppleSearchAds, status, &text, None));
            }
            return Ok(AdMutationResult {
                dry_run: false,
                accepted: true,
                platform: PlatformId::AppleSearchAds,
                ad_account_id: account.id.clone(),
                affected_entity_ids: request.entity_ids.clone(),
                summary: "Apple Search Ads live mutation submitted".into(),
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
        let status_val = if request.initial_status.eq_ignore_ascii_case("PAUSED") {
            "PAUSED"
        } else {
            "ENABLED"
        };
        let (path, mut body) = match request.entity_kind {
            AdCreationEntityKind::Campaign => {
                let mut p = json!({
                    "orgId": account.platform_account_id.parse::<i64>().unwrap_or(0),
                    "name": request.name,
                    "adamId": 6449102831i64,
                    "countriesOrRegions": ["US"],
                    "status": status_val,
                });
                if let Some(b_fen) = request.daily_budget_fen {
                    p["dailyBudgetAmount"] = json!({
                        "amount": format!("{:.2}", base_fen_to_currency_major(b_fen, fx_rate)),
                        "currency": currency,
                    });
                }
                ("/campaigns".to_string(), p)
            }
            AdCreationEntityKind::AdGroup => {
                let cid = request.parent_entity_id.clone().ok_or_else(|| {
                    ConnectorError::InvalidRequest {
                        platform: PlatformId::AppleSearchAds,
                        message: "ASA adgroup creation requires parent_entity_id (campaignId)".into(),
                    }
                })?;
                let mut p = json!({
                    "campaignId": cid,
                    "name": request.name,
                    "status": status_val,
                });
                if let Some(bid_fen) = request.bid_fen {
                    p["defaultBidAmount"] = json!({
                        "amount": format!("{:.2}", base_fen_to_currency_major(bid_fen, fx_rate)),
                        "currency": currency,
                    });
                }
                (format!("/campaigns/{cid}/adgroups"), p)
            }
            AdCreationEntityKind::Creative => {
                let ag_id = request.parent_entity_id.clone().ok_or_else(|| {
                    ConnectorError::InvalidRequest {
                        platform: PlatformId::AppleSearchAds,
                        message: "ASA ad creation requires parent_entity_id (adGroupId)".into(),
                    }
                })?;
                let p = json!({
                    "name": request.name,
                    "status": status_val,
                    "creativeType": "CUSTOM_PRODUCT_PAGE",
                    "creativeAssets": request.creative_assets,
                });
                (format!("/campaigns/0/adgroups/{ag_id}/ads"), p)
            }
        };
        merge_json_extra(&mut body, &request.platform_extra);
        let note = format!(
            "Apple Search Ads v5 uses `{currency}` string `amount` + `currency` objects and requires `X-AP-Context: orgId=...`."
        );
        let risk_warnings = platform_creation_notes(PlatformId::AppleSearchAds, Some(&note));

        if request.dry_run || matches!(self.mode, ConnectorMode::Mock) {
            let created_id = if request.dry_run {
                None
            } else {
                Some(format!(
                    "asa_new_{}",
                    &Uuid::new_v4().simple().to_string()[..8]
                ))
            };
            return Ok(AdCreationResult {
                dry_run: request.dry_run,
                accepted: true,
                platform: PlatformId::AppleSearchAds,
                ad_account_id: account.id.clone(),
                entity_kind: request.entity_kind,
                created_entity_id: created_id.clone(),
                initial_status: request.initial_status.clone(),
                summary: format!(
                    "Apple Search Ads v5 {:?} creation {} ({})",
                    request.entity_kind,
                    if request.dry_run { "previewed" } else { "created in mock" },
                    self.mode_label()
                ),
                risk_warnings,
                guardrail_notes: vec![format!("Target endpoint: POST {}{}", self.base_url(), path)],
                platform_payload_preview: body,
                platform_response: json!({ "data": { "id": created_id } }),
            });
        }

        if let ConnectorMode::Live(creds) = &self.mode {
            let url = format!("{}{}", self.base_url(), path);
            let org_ctx = if creds.org_id.is_empty() {
                format!("orgId={}", account.platform_account_id)
            } else {
                format!("orgId={}", creds.org_id)
            };
            let resp = self
                .client
                .post(&url)
                .bearer_auth(&creds.access_token)
                .header("X-AP-Context", org_ctx)
                .json(&body)
                .send()
                .await
                .map_err(|e| ConnectorError::Transient {
                    platform: PlatformId::AppleSearchAds,
                    message: e.to_string(),
                })?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(map_http_status(PlatformId::AppleSearchAds, status, &text, None));
            }
            let val: Value = serde_json::from_str(&text).unwrap_or(json!({"raw": text}));
            let data = val.get("data").unwrap_or(&val);
            let created_id = json_opt_str(data, "id");
            return Ok(AdCreationResult {
                dry_run: false,
                accepted: true,
                platform: PlatformId::AppleSearchAds,
                ad_account_id: account.id.clone(),
                entity_kind: request.entity_kind,
                created_entity_id: created_id,
                initial_status: request.initial_status.clone(),
                summary: format!("Apple Search Ads live {:?} created", request.entity_kind),
                risk_warnings,
                guardrail_notes: vec![],
                platform_payload_preview: body,
                platform_response: val,
            });
        }
        unreachable!()
    }
}

pub struct AppleSearchAdsPlugin {
    connector: Arc<AppleSearchAdsConnector>,
}

impl AppleSearchAdsPlugin {
    pub fn new(mode: ConnectorMode) -> Self {
        Self {
            connector: Arc::new(AppleSearchAdsConnector::new(mode)),
        }
    }
}

#[async_trait]
impl Plugin for AppleSearchAdsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "connector.apple_search_ads".into(),
            name: "Apple Search Ads Connector (API v5 + ES256)".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Official Apple Search Ads API v5 connector with pure-Rust ES256 JWT signing and FX normalization".into(),
            category: PluginCategory::Connector,
            provides: vec!["connector:apple_search_ads".into()],
            requires: vec!["storage".into()],
            config_schema: json!({"type": "object"}),
        }
    }

    async fn mount(&self, ctx: &mut PluginContext) -> KernelResult<()> {
        let svc: Arc<dyn AdPlatformConnector> =
            Arc::clone(&self.connector) as Arc<dyn AdPlatformConnector>;
        ctx.provide_named_service(PlatformId::AppleSearchAds.as_str(), svc);
        Ok(())
    }
}
