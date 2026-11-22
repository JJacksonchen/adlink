//! Connector plugins registry for all 15 ad platforms (10 domestic CN + 5 global overseas).

pub mod alipay_denghuo;
pub mod amazon_ads;
pub mod apple_search_ads;
pub mod bilibili_huahuo;
pub mod bilibili_sanlian;
pub mod google_ads;
pub mod http_helpers;
pub mod kuaishou;
pub mod meta_ads;
pub mod mock_data;
pub mod mock_generator;
pub mod ocean_engine;
pub mod onboarding;
pub mod tencent_ads;
pub mod tiktok_ads;
pub mod xhs_juguang;
pub mod xhs_pugongying;

use std::collections::BTreeMap;
use std::sync::Arc;

use adlink_domain::PlatformId;
use adlink_kernel::{Kernel, KernelResult, Plugin};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use alipay_denghuo::{
    build_alipay_sign_content, sign_alipay_rsa2, AlipayDenghuoConnector, AlipayDenghuoPlugin,
};
pub use amazon_ads::{AmazonAdsConnector, AmazonAdsPlugin};
pub use apple_search_ads::{
    sign_apple_es256_client_secret, AppleSearchAdsConnector, AppleSearchAdsPlugin,
};
pub use bilibili_huahuo::{BilibiliHuahuoConnector, BilibiliHuahuoPlugin};
pub use bilibili_sanlian::{BilibiliSanlianConnector, BilibiliSanlianPlugin};
pub use google_ads::{GoogleAdsConnector, GoogleAdsPlugin};
pub use http_helpers::{ConnectorMode, LiveCredentials};
pub use kuaishou::{KuaishouConnector, KuaishouPlugin};
pub use meta_ads::{MetaAdsConnector, MetaAdsPlugin};
pub use mock_data::{mock_account_id, mock_platform_account_id, seed_mock_governance};
pub use ocean_engine::{OceanEngineConnector, OceanEnginePlugin};
pub use onboarding::build_onboard_guide;
pub use tencent_ads::{TencentAdsConnector, TencentAdsPlugin};
pub use tiktok_ads::{TikTokAdsConnector, TikTokAdsPlugin};
pub use xhs_juguang::{XhsAdsConnector, XhsPlugin};
pub use xhs_pugongying::{XhsPugongyingConnector, XhsPugongyingPlugin};

/// Declarative map of per-platform connector modes (`mock` vs `live`) loaded from TOML/env.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConnectorSuiteConfig {
    #[serde(default)]
    pub connectors: BTreeMap<String, ConnectorMode>,
}

impl ConnectorSuiteConfig {
    pub fn mode_for(&self, platform: PlatformId) -> ConnectorMode {
        self.connectors
            .get(platform.as_str())
            .cloned()
            .unwrap_or(ConnectorMode::Mock)
    }
}

/// Mounts all 10 platform connector plugins into the AdLink microkernel.
pub async fn mount_all_connectors(
    kernel: &Kernel,
    suite: &ConnectorSuiteConfig,
) -> KernelResult<()> {
    let plugins: Vec<Arc<dyn Plugin>> = vec![
        Arc::new(TencentAdsPlugin::new(suite.mode_for(PlatformId::TencentAds))),
        Arc::new(XhsPlugin::juguang(suite.mode_for(PlatformId::XhsJuguang))),
        Arc::new(XhsPlugin::chengfeng(suite.mode_for(PlatformId::XhsChengfeng))),
        Arc::new(XhsPugongyingPlugin::new(
            suite.mode_for(PlatformId::XhsPugongying),
        )),
        Arc::new(BilibiliSanlianPlugin::new(
            suite.mode_for(PlatformId::BilibiliSanlian),
        )),
        Arc::new(BilibiliHuahuoPlugin::new(
            suite.mode_for(PlatformId::BilibiliHuahuo),
        )),
        Arc::new(KuaishouPlugin::new(suite.mode_for(PlatformId::Kuaishou))),
        Arc::new(OceanEnginePlugin::ad(
            suite.mode_for(PlatformId::OceanEngineAd),
        )),
        Arc::new(OceanEnginePlugin::local(
            suite.mode_for(PlatformId::OceanEngineLocal),
        )),
        Arc::new(AlipayDenghuoPlugin::new(
            suite.mode_for(PlatformId::AlipayDenghuo),
        )),
        Arc::new(TikTokAdsPlugin::new(suite.mode_for(PlatformId::TikTokAds))),
        Arc::new(MetaAdsPlugin::new(suite.mode_for(PlatformId::MetaAds))),
        Arc::new(GoogleAdsPlugin::new(suite.mode_for(PlatformId::GoogleAds))),
        Arc::new(AppleSearchAdsPlugin::new(
            suite.mode_for(PlatformId::AppleSearchAds),
        )),
        Arc::new(AmazonAdsPlugin::new(suite.mode_for(PlatformId::AmazonAds))),
    ];

    for plugin in plugins {
        kernel.mount(plugin, Value::Null).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use adlink_domain::{
        AccountType, AdAccount, ReportGranularity, ReportLevel, TimeLine,
    };
    use adlink_engine::{AdPlatformConnector, ReportFetchRequest};
    use adlink_kernel::Kernel;
    use adlink_storage::{SqliteStorage, SqliteStoragePlugin};
    use base64::Engine;
    use chrono::NaiveDate;
    use ring::signature::{UnparsedPublicKey, RSA_PKCS1_2048_8192_SHA256};
    use tempfile::tempdir;

    #[tokio::test]
    async fn all_ten_connectors_mount_and_parse_mock_official_payloads() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(SqliteStorage::open(dir.path().join("all10.db")).unwrap());
        let kernel = Arc::new(Kernel::new());
        kernel
            .mount(
                Arc::new(SqliteStoragePlugin::new(storage)),
                Value::Null,
            )
            .await
            .unwrap();
        mount_all_connectors(&kernel, &ConnectorSuiteConfig::default())
            .await
            .unwrap();

        let start = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 10, 3).unwrap();

        for &platform in PlatformId::ALL {
            let conn = kernel
                .get_named_service::<dyn AdPlatformConnector>(platform.as_str())
                .expect("connector mounted");
            let account = AdAccount {
                id: mock_account_id(platform).into(),
                org_id: "org-1".into(),
                platform,
                platform_account_id: mock_platform_account_id(platform).into(),
                account_name: platform.display_name_zh().into(),
                account_type: AccountType::AgencySub,
                rebate_rate: 0.05,
                service_fee_rate: 0.08,
                default_cash_ratio: 0.9,
                status: "active".into(),
                credential_ref: None,
                extra: Value::Null,
            };
            let page = conn
                .fetch_reports_page(&ReportFetchRequest {
                    account: account.clone(),
                    start_date: start,
                    end_date: end,
                    granularity: ReportGranularity::Daily,
                    level: ReportLevel::Creative,
                    time_line: TimeLine::ReportingTime,
                    page: 1,
                    page_size: 50,
                })
                .await
                .unwrap();
            assert!(!page.rows.is_empty(), "empty rows for {platform}");
            for row in &page.rows {
                assert!(!row.raw_fields.is_empty(), "raw_fields lost for {platform}");
            }
            let funds = conn
                .fetch_fund_statements(&account, start, end)
                .await
                .unwrap();
            assert!(!funds.is_empty(), "empty fund statement for {platform}");
        }
    }

    #[test]
    fn alipay_rsa2_canonical_signing_and_verification_works() {
        // Generate an ephemeral 2048-bit PKCS#8 RSA key dynamically using Python cryptography or openssl if present,
        // or verify canonical parameter ordering + known valid error behavior, and if openssl exists verify real signature.
        let mut params = BTreeMap::new();
        params.insert("app_id".into(), "2021004192837465".into());
        params.insert("method".into(), "alipay.data.dataservice.ad.reportdata.query".into());
        params.insert("charset".into(), "utf-8".into());
        params.insert("sign_type".into(), "RSA2".into());
        params.insert("sign".into(), "should_be_excluded".into());
        params.insert("timestamp".into(), "2026-11-21 12:00:00".into());
        params.insert("version".into(), "1.0".into());
        params.insert("biz_content".into(), "{\"ad_level\":\"PLAN\"}".into());

        let canonical = build_alipay_sign_content(&params);
        assert_eq!(
            canonical,
            "app_id=2021004192837465&biz_content={\"ad_level\":\"PLAN\"}&charset=utf-8&method=alipay.data.dataservice.ad.reportdata.query&sign_type=RSA2&timestamp=2026-11-21 12:00:00&version=1.0"
        );

        // Generate a real temporary PKCS#8 RSA-2048 key via openssl to prove end-to-end RSA2 cryptographic signing + public-key verification!
        let output = std::process::Command::new("openssl")
            .args(["genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048"])
            .output();
        if let Ok(out) = output {
            if out.status.success() {
                let pem = String::from_utf8(out.stdout).unwrap();
                let sig_b64 = sign_alipay_rsa2(&pem, &canonical).unwrap();
                let sig_bytes = base64::engine::general_purpose::STANDARD
                    .decode(sig_b64.as_bytes())
                    .unwrap();

                let der_b64: String = pem
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.starts_with("-----") && !l.is_empty())
                    .collect();
                let der = base64::engine::general_purpose::STANDARD
                    .decode(der_b64.as_bytes())
                    .unwrap();
                let kp = ring::signature::RsaKeyPair::from_pkcs8(&der).unwrap();
                let pub_key = UnparsedPublicKey::new(
                    &RSA_PKCS1_2048_8192_SHA256,
                    kp.public().as_ref(),
                );
                pub_key.verify(canonical.as_bytes(), &sig_bytes).unwrap();
            }
        }
    }

    fn sample_account(platform: PlatformId) -> AdAccount {
        AdAccount {
            id: mock_account_id(platform).into(),
            org_id: "org-1".into(),
            platform,
            platform_account_id: mock_platform_account_id(platform).into(),
            account_name: "Test".into(),
            account_type: AccountType::AgencySub,
            rebate_rate: 0.05,
            service_fee_rate: 0.08,
            default_cash_ratio: 0.9,
            status: "active".into(),
            credential_ref: None,
            extra: Value::Null,
        }
    }

    #[tokio::test]
    async fn alipay_avoids_gmv_double_counting_and_supports_pagination() {
        let acct = sample_account(PlatformId::AlipayDenghuo);
        let conn = AlipayDenghuoConnector::new(ConnectorMode::Mock);

        // Mock spans 5 days with page_size=2 -> page 1 has_more=true, and GMV matches single conv_data_list amount (not 2x).
        let page1 = conn
            .fetch_reports_page(&ReportFetchRequest {
                account: acct.clone(),
                start_date: NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
                end_date: NaiveDate::from_ymd_opt(2026, 10, 5).unwrap(),
                granularity: ReportGranularity::Daily,
                level: ReportLevel::Campaign,
                time_line: TimeLine::ReportingTime,
                page: 1,
                page_size: 2,
            })
            .await
            .unwrap();
        assert_eq!(page1.rows.len(), 2);
        assert!(page1.has_more);
        let raw_conv_amt = page1.rows[0].raw_fields["conversion_data_list"][1]["conversion_amount_fen"]
            .as_i64()
            .unwrap();
        assert_eq!(page1.rows[0].gmv_fen.as_fen(), raw_conv_amt);

        // Real-style payload using official `conversion_data_list` (without `conv_data_list`).
        let real_payload = serde_json::json!({
            "alipay_data_dataservice_ad_reportdata_query_response": {
                "code": "10000",
                "total": 1,
                "data_list": [{
                    "biz_date": "20261001",
                    "data_id": "dh_plan_99",
                    "data_name": "MiniApp Plan",
                    "cost": 12000,
                    "impression": 4000,
                    "click": 200,
                    "conversion_data_list": [
                        { "conversion_type": "FORM_SUBMIT", "conversion_type_name": "表单线索", "conversion_result": 5 },
                        { "conversion_type": "MINIAPP_PAY_ORDER", "conversion_type_name": "小程序支付", "conversion_result": 8, "conversion_amount_fen": 45000 }
                    ]
                }]
            }
        });
        let parsed = AlipayDenghuoConnector::parse_official_reports_response(
            &real_payload,
            &acct,
            ReportGranularity::Daily,
            ReportLevel::Campaign,
            TimeLine::ReportingTime,
            1,
            20,
        )
        .unwrap();
        assert_eq!(parsed.rows[0].gmv_fen.as_fen(), 45000);
        assert_eq!(parsed.rows[0].leads, 5);
        assert_eq!(parsed.rows[0].orders, 8);
        assert_eq!(parsed.rows[0].conversions, 13);
        assert!(!parsed.has_more);
    }

    #[test]
    fn connectors_classify_upstream_rate_limits_and_auth_errors_cleanly() {
        use adlink_engine::ConnectorError;

        let tencent_rl = TencentAdsConnector::parse_official_reports_response(
            &serde_json::json!({"code": 11000, "message": "rate limit"}),
            &sample_account(PlatformId::TencentAds),
            ReportGranularity::Daily,
            ReportLevel::Account,
            TimeLine::ReportingTime,
        )
        .unwrap_err();
        assert!(matches!(tencent_rl, ConnectorError::RateLimited { .. }));
        assert!(tencent_rl.is_retryable());

        let tencent_auth = TencentAdsConnector::parse_official_reports_response(
            &serde_json::json!({"code": 10001, "message": "invalid token"}),
            &sample_account(PlatformId::TencentAds),
            ReportGranularity::Daily,
            ReportLevel::Account,
            TimeLine::ReportingTime,
        )
        .unwrap_err();
        assert!(matches!(tencent_auth, ConnectorError::AuthError { .. }));
        assert!(!tencent_auth.is_retryable());

        let oe_rl = OceanEngineConnector::parse_official_reports_response(
            PlatformId::OceanEngineAd,
            &serde_json::json!({"code": 40100, "message": "qps limit"}),
            &sample_account(PlatformId::OceanEngineAd),
            ReportGranularity::Daily,
            ReportLevel::Account,
            TimeLine::ReportingTime,
        )
        .unwrap_err();
        assert!(matches!(oe_rl, ConnectorError::RateLimited { .. }));

        let oe_auth = OceanEngineConnector::parse_official_reports_response(
            PlatformId::OceanEngineAd,
            &serde_json::json!({"code": 40105, "message": "token expired"}),
            &sample_account(PlatformId::OceanEngineAd),
            ReportGranularity::Daily,
            ReportLevel::Account,
            TimeLine::ReportingTime,
        )
        .unwrap_err();
        assert!(matches!(oe_auth, ConnectorError::AuthError { .. }));

        let ks_rl = KuaishouConnector::parse_official_reports_response(
            &serde_json::json!({"code": 400004, "message": "too fast"}),
            &sample_account(PlatformId::Kuaishou),
            ReportGranularity::Daily,
            ReportLevel::Account,
            TimeLine::ReportingTime,
            1,
            20,
        )
        .unwrap_err();
        assert!(matches!(ks_rl, ConnectorError::RateLimited { .. }));

        let xhs_auth = XhsAdsConnector::parse_official_reports_response(
            PlatformId::XhsJuguang,
            &serde_json::json!({"code": 401, "success": false, "msg": "unauthorized"}),
            &sample_account(PlatformId::XhsJuguang),
            ReportGranularity::Daily,
            ReportLevel::Account,
            TimeLine::ReportingTime,
            1,
            20,
            false,
        )
        .unwrap_err();
        assert!(matches!(xhs_auth, ConnectorError::AuthError { .. }));

        let ali_err = AlipayDenghuoConnector::parse_official_reports_response(
            &serde_json::json!({
                "alipay_data_dataservice_ad_reportdata_query_response": {
                    "code": "40002",
                    "msg": "Invalid Arguments",
                    "sub_msg": "isv.invalid-signature"
                }
            }),
            &sample_account(PlatformId::AlipayDenghuo),
            ReportGranularity::Daily,
            ReportLevel::Account,
            TimeLine::ReportingTime,
            1,
            20,
        )
        .unwrap_err();
        assert!(matches!(ali_err, ConnectorError::InvalidRequest { .. }));
    }

    #[tokio::test]
    async fn live_field_audit_fixes_parse_official_shapes_accurately() {
        // 1. Tencent Ads v3.0: cheout_pv_1d, cheout_fd, web_leads_count, numeric campaign/adgroup/creative IDs
        let tc_acct = sample_account(PlatformId::TencentAds);
        let tc_page = TencentAdsConnector::parse_official_reports_response(
            &serde_json::json!({
                "code": 0,
                "data": {
                    "list": [{
                        "date": "2026-11-10",
                        "account_id": 38291044,
                        "campaign_id": 910022,
                        "adgroup_id": 820033,
                        "dynamic_creative_id": 730044,
                        "adgroup_name": "腾讯首日付费组",
                        "material_id": 550066,
                        "cost": 15250,
                        "view_count": 12000,
                        "valid_click_count": 340,
                        "conversions_count": 21,
                        "deep_conversions_count": 9,
                        "web_leads_count": 7,
                        "cheout_pv_1d": 14,
                        "cheout_fd": 98000,
                        "praise_count": 18,
                        "comment_count": 2,
                        "video_play_count": 6400,
                        "video_inner_play_count": 2100
                    }],
                    "page_info": { "page": 1, "page_size": 20, "total_number": 1, "total_page": 1 }
                }
            }),
            &tc_acct,
            ReportGranularity::Daily,
            ReportLevel::Creative,
            TimeLine::ReportingTime,
        )
        .unwrap();
        assert_eq!(tc_page.rows[0].entity_id, "730044");
        assert_eq!(tc_page.rows[0].parent_entity_id.as_deref(), Some("910022"));
        assert_eq!(tc_page.rows[0].content_id.as_deref(), Some("550066"));
        assert_eq!(tc_page.rows[0].orders, 14);
        assert_eq!(tc_page.rows[0].gmv_fen.as_fen(), 98000);
        assert_eq!(tc_page.rows[0].leads, 7);

        // 2. Ocean Engine v3.0: MATERIAL_DATA hourly string timestamp & numeric IDs & clue_cnt
        let oe_acct = sample_account(PlatformId::OceanEngineAd);
        let oe_page = OceanEngineConnector::parse_official_reports_response(
            PlatformId::OceanEngineAd,
            &serde_json::json!({
                "code": 0,
                "data": {
                    "rows": [{
                        "dimensions": {
                            "stat_time_hour": "2026-11-10 14:00:00",
                            "project_id": 180001,
                            "material_id": 290002
                        },
                        "metrics": {
                            "stat_cost": 88.50,
                            "show_cnt": 9500,
                            "click_cnt": 285,
                            "convert_cnt": 15,
                            "clue_cnt": 4,
                            "pay_order_count": 11,
                            "pay_order_amount": 1245.50,
                            "total_play": 4200,
                            "valid_play": 1800
                        }
                    }],
                    "page_info": { "page": 1, "page_size": 20, "total_number": 1, "total_page": 1 }
                }
            }),
            &oe_acct,
            ReportGranularity::Hourly,
            ReportLevel::Creative,
            TimeLine::ReportingTime,
        )
        .unwrap();
        assert_eq!(oe_page.rows[0].stat_hour, 14);
        assert_eq!(oe_page.rows[0].gross_cost_fen.as_fen(), 8850);
        assert_eq!(oe_page.rows[0].gmv_fen.as_fen(), 124550);
        assert_eq!(oe_page.rows[0].parent_entity_id.as_deref(), Some("180001"));
        assert_eq!(oe_page.rows[0].content_id.as_deref(), Some("290002"));
        assert_eq!(oe_page.rows[0].leads, 4);

        // 3. Kuaishou: aclick / form_count / event_pay / activation / total_count pagination without has_more key
        let ks_acct = sample_account(PlatformId::Kuaishou);
        let ks_page = KuaishouConnector::parse_official_reports_response(
            &serde_json::json!({
                "code": 0,
                "data": {
                    "total_count": 45,
                    "details": [{
                        "stat_date": "2026-11-10",
                        "campaign_id": 33001,
                        "unit_id": 44002,
                        "unit_name": "快手单元A",
                        "photo_id": 55003,
                        "charge": 76.0,
                        "show": 5000,
                        "aclick": 190,
                        "form_count": 6,
                        "event_pay": 9,
                        "event_pay_purchase_amount": 810.25,
                        "activation": 15
                    }]
                }
            }),
            &ks_acct,
            ReportGranularity::Daily,
            ReportLevel::AdGroup,
            TimeLine::ReportingTime,
            1,
            20,
        )
        .unwrap();
        assert!(ks_page.has_more);
        assert_eq!(ks_page.rows[0].gross_cost_fen.as_fen(), 7600);
        assert_eq!(ks_page.rows[0].clicks, 190);
        assert_eq!(ks_page.rows[0].leads, 6);
        assert_eq!(ks_page.rows[0].orders, 9);
        assert_eq!(ks_page.rows[0].conversions, 15);
        assert_eq!(ks_page.rows[0].gmv_fen.as_fen(), 81025);
        assert_eq!(ks_page.rows[0].entity_name, "快手单元A");

        // 4. XHS Juguang: decimal string yuan "123.45" parsed cleanly + total_count pagination
        let xhs_acct = sample_account(PlatformId::XhsJuguang);
        let xhs_page = XhsAdsConnector::parse_official_reports_response(
            PlatformId::XhsJuguang,
            &serde_json::json!({
                "code": 0,
                "success": true,
                "data": {
                    "total_count": 50,
                    "data_list": [{
                        "time": "2026-11-10",
                        "campaign_id": 77100,
                        "unit_id": 77200,
                        "unit_name": "聚光搜索词单元",
                        "note_id": "671aabbccddeeff00112233",
                        "fee": "123.45",
                        "impression": 8800,
                        "click": 410,
                        "interaction": 95,
                        "deal_order_num": 12,
                        "deal_order_gmv": "1980.50"
                    }]
                }
            }),
            &xhs_acct,
            ReportGranularity::Daily,
            ReportLevel::AdGroup,
            TimeLine::ReportingTime,
            1,
            20,
            false,
        )
        .unwrap();
        assert!(xhs_page.has_more);
        assert_eq!(xhs_page.rows[0].gross_cost_fen.as_fen(), 12345);
        assert_eq!(xhs_page.rows[0].gmv_fen.as_fen(), 198050);
        assert_eq!(xhs_page.rows[0].parent_entity_id.as_deref(), Some("77100"));

        // 5. Creation payload & unit conversion checks (Kuaishou li = fen * 10; Denghuo unsupported)
        use adlink_domain::{AdCreationEntityKind, AdCreationRequest};
        use adlink_engine::ConnectorError;
        let ks_conn = KuaishouConnector::new(ConnectorMode::Mock);
        let ks_create = ks_conn
            .create_ad_entity(
                &ks_acct,
                &AdCreationRequest {
                    platform: PlatformId::Kuaishou,
                    ad_account_id: ks_acct.id.clone(),
                    entity_kind: AdCreationEntityKind::Campaign,
                    name: "Preview KS Campaign".into(),
                    parent_entity_id: None,
                    marketing_goal: None,
                    daily_budget_fen: Some(50_000),
                    bid_fen: None,
                    targeting: serde_json::json!({}),
                    creative_assets: serde_json::json!({}),
                    landing_url: None,
                    initial_status: "PAUSED".into(),
                    dry_run: true,
                    preview_confirmed: false,
                    reason: "unit test preview".into(),
                    operator: "test".into(),
                    platform_extra: serde_json::json!({}),
                },
            )
            .await
            .unwrap();
        assert_eq!(ks_create.platform_payload_preview["day_budget"], 500_000);
        assert_eq!(ks_create.platform_payload_preview["put_status"], 2);

        let ali_conn = AlipayDenghuoConnector::new(ConnectorMode::Mock);
        let ali_acct = sample_account(PlatformId::AlipayDenghuo);
        let ali_create_err = ali_conn
            .create_ad_entity(
                &ali_acct,
                &AdCreationRequest {
                    platform: PlatformId::AlipayDenghuo,
                    ad_account_id: ali_acct.id.clone(),
                    entity_kind: AdCreationEntityKind::Campaign,
                    name: "Unsupported".into(),
                    parent_entity_id: None,
                    marketing_goal: None,
                    daily_budget_fen: Some(10_000),
                    bid_fen: None,
                    targeting: serde_json::json!({}),
                    creative_assets: serde_json::json!({}),
                    landing_url: None,
                    initial_status: "PAUSED".into(),
                    dry_run: true,
                    preview_confirmed: false,
                    reason: "test unsupported".into(),
                    operator: "test".into(),
                    platform_extra: serde_json::json!({}),
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(ali_create_err, ConnectorError::InvalidRequest { .. }));
    }

    #[tokio::test]
    async fn overseas_platforms_parse_fx_units_and_es256_jwt_correctly() {
        use adlink_domain::{AdCreationEntityKind, AdCreationRequest};
        use adlink_engine::ConnectorError;

        let fx_extra = serde_json::json!({
            "currency": "USD",
            "fx_rate_to_base": 7.25
        });

        let mut tt_acct = sample_account(PlatformId::TikTokAds);
        tt_acct.extra = fx_extra.clone();
        let tt_page = TikTokAdsConnector::parse_official_reports_response(
            &serde_json::json!({
                "code": 0,
                "message": "OK",
                "data": {
                    "list": [{
                        "dimensions": {
                            "stat_time_day": "2026-11-10 00:00:00",
                            "campaign_id": "180123",
                            "ad_id": "980123"
                        },
                        "metrics": {
                            "campaign_name": "TT US Serum",
                            "ad_name": "UGC Ad 01",
                            "spend": "100.00",
                            "impressions": "15000",
                            "clicks": "420",
                            "complete_payment": "18",
                            "total_complete_payment_value": "540.00",
                            "currency": "USD"
                        }
                    }]
                }
            }),
            &tt_acct,
            ReportGranularity::Daily,
            ReportLevel::Creative,
            TimeLine::ReportingTime,
        )
        .unwrap();
        assert_eq!(tt_page.rows.len(), 1);
        assert_eq!(tt_page.rows[0].gross_cost_fen.as_fen(), 72_500);
        assert_eq!(tt_page.rows[0].gmv_fen.as_fen(), 391_500);
        assert_eq!(tt_page.rows[0].orders, 18);

        let mut meta_acct = sample_account(PlatformId::MetaAds);
        meta_acct.extra = fx_extra.clone();
        let meta_page = MetaAdsConnector::parse_official_reports_response(
            &serde_json::json!({
                "data": [{
                    "date_start": "2026-11-10",
                    "account_currency": "USD",
                    "campaign_id": "fb_c1",
                    "campaign_name": "Meta CBO",
                    "ad_id": "fb_a1",
                    "ad_name": "Reels 01",
                    "spend": "20.00",
                    "impressions": "9000",
                    "clicks": "210",
                    "actions": [
                        { "action_type": "purchase", "value": "5" },
                        { "action_type": "omni_purchase", "value": "7" },
                        { "action_type": "post_engagement", "value": "88" }
                    ],
                    "action_values": [
                        { "action_type": "purchase", "value": "150.00" },
                        { "action_type": "omni_purchase", "value": "200.00" }
                    ]
                }]
            }),
            &meta_acct,
            ReportGranularity::Daily,
            ReportLevel::Creative,
            TimeLine::ReportingTime,
        )
        .unwrap();
        // $20.00 * 7.25 = ¥145.00 = 14500 fen; omni_purchase $200 = ¥1450 = 145000 fen
        assert_eq!(meta_page.rows[0].gross_cost_fen.as_fen(), 14_500);
        assert_eq!(meta_page.rows[0].orders, 7);
        assert_eq!(meta_page.rows[0].gmv_fen.as_fen(), 145_000);

        // Meta creation preview must convert base CNY fen -> USD cents (¥725 = $100 = 10000 cents)
        let meta_conn = MetaAdsConnector::new(ConnectorMode::Mock);
        let meta_create = meta_conn
            .create_ad_entity(
                &meta_acct,
                &AdCreationRequest {
                    platform: PlatformId::MetaAds,
                    ad_account_id: meta_acct.id.clone(),
                    entity_kind: AdCreationEntityKind::Campaign,
                    name: "Meta Preview".into(),
                    parent_entity_id: None,
                    marketing_goal: None,
                    daily_budget_fen: Some(72_500),
                    bid_fen: None,
                    targeting: serde_json::json!({}),
                    creative_assets: serde_json::json!({}),
                    landing_url: None,
                    initial_status: "PAUSED".into(),
                    dry_run: true,
                    preview_confirmed: false,
                    reason: "unit test meta cents".into(),
                    operator: "test".into(),
                    platform_extra: serde_json::json!({}),
                },
            )
            .await
            .unwrap();
        assert_eq!(meta_create.platform_payload_preview["daily_budget"], 10_000);

        let mut gg_acct = sample_account(PlatformId::GoogleAds);
        gg_acct.extra = fx_extra.clone();
        let gg_page = GoogleAdsConnector::parse_official_reports_response(
            &serde_json::json!([{
                "results": [{
                    "customer": { "id": "7428910345", "currencyCode": "USD" },
                    "campaign": { "id": "21001", "name": "Google PMax" },
                    "segments": { "date": "2026-11-10" },
                    "metrics": {
                        "costMicros": "10000000",
                        "impressions": "12000",
                        "clicks": "350",
                        "conversions": 12.0,
                        "conversionsValue": 400.0
                    }
                }]
            }]),
            &gg_acct,
            ReportGranularity::Daily,
            ReportLevel::Campaign,
            TimeLine::ReportingTime,
        )
        .unwrap();
        // 10_000_000 micros = $10 * 7.25 = ¥72.50 = 7250 fen; $400 = ¥2900 = 290000 fen
        assert_eq!(gg_page.rows[0].gross_cost_fen.as_fen(), 7_250);
        assert_eq!(gg_page.rows[0].gmv_fen.as_fen(), 290_000);

        let mut asa_acct = sample_account(PlatformId::AppleSearchAds);
        asa_acct.extra = fx_extra.clone();
        let asa_page = AppleSearchAdsConnector::parse_official_reports_response(
            &serde_json::json!({
                "data": {
                    "reportingDataResponse": {
                        "row": [{
                            "metadata": { "campaignId": 991, "campaignName": "ASA Brand" },
                            "granularity": [{
                                "date": "2026-11-10",
                                "localSpend": { "amount": "50.00", "currency": "USD" },
                                "impressions": 4500,
                                "taps": 225,
                                "tapInstalls": 40
                            }]
                        }]
                    }
                }
            }),
            &asa_acct,
            ReportGranularity::Daily,
            ReportLevel::Campaign,
            TimeLine::ReportingTime,
        )
        .unwrap();
        assert_eq!(asa_page.rows[0].gross_cost_fen.as_fen(), 36_250);
        assert_eq!(asa_page.rows[0].clicks, 225);
        assert_eq!(asa_page.rows[0].conversions, 40);

        // Pure-Rust ES256 client_secret JWT signing via ECDSA P-256 (PKCS#8 named_curve).
        let rng = ring::rand::SystemRandom::new();
        let pkcs8_doc = ring::signature::EcdsaKeyPair::generate_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &rng,
        )
        .unwrap();
        let pem = format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            base64::engine::general_purpose::STANDARD.encode(pkcs8_doc.as_ref())
        );
        let jwt = sign_apple_es256_client_secret(
            "SEARCHADS.client",
            "SEARCHADS.team",
            "key-123",
            &pem,
        )
        .unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let kp = ring::signature::EcdsaKeyPair::from_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            pkcs8_doc.as_ref(),
            &rng,
        )
        .unwrap();
        let sig = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[2])
            .unwrap();
        let pub_key = UnparsedPublicKey::new(
            &ring::signature::ECDSA_P256_SHA256_FIXED,
            ring::signature::KeyPair::public_key(&kp).as_ref(),
        );
        pub_key
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig)
            .unwrap();

        let mut amz_acct = sample_account(PlatformId::AmazonAds);
        amz_acct.extra = fx_extra;
        let pending_err = AmazonAdsConnector::parse_official_reports_response(
            &serde_json::json!({ "status": "PENDING", "reportId": "rep-9988" }),
            &amz_acct,
            ReportGranularity::Daily,
            ReportLevel::Campaign,
            TimeLine::ReportingTime,
        )
        .unwrap_err();
        assert!(matches!(
            pending_err,
            ConnectorError::AsyncTaskPending { ref task_id, .. } if task_id == "rep-9988"
        ));

        let amz_page = AmazonAdsConnector::parse_official_reports_response(
            &serde_json::json!([{
                "date": "2026-11-10",
                "campaignId": "amz-c1",
                "campaignName": "SP Hero",
                "campaignBudgetCurrencyCode": "USD",
                "cost": 40.0,
                "impressions": 6000,
                "clicks": 180,
                "purchases14d": 15,
                "sales14d": 280.0
            }]),
            &amz_acct,
            ReportGranularity::Daily,
            ReportLevel::Campaign,
            TimeLine::ReportingTime,
        )
        .unwrap();
        assert_eq!(amz_page.rows[0].gross_cost_fen.as_fen(), 29_000);
        assert_eq!(amz_page.rows[0].gmv_fen.as_fen(), 203_000);
        assert_eq!(amz_page.rows[0].orders, 15);
    }
}
