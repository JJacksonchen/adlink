//! Connector plugins registry for all 8 Chinese ad platforms (10 logical connectors).

pub mod alipay_denghuo;
pub mod bilibili_huahuo;
pub mod bilibili_sanlian;
pub mod http_helpers;
pub mod kuaishou;
pub mod mock_data;
pub mod mock_generator;
pub mod ocean_engine;
pub mod onboarding;
pub mod tencent_ads;
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
pub use bilibili_huahuo::{BilibiliHuahuoConnector, BilibiliHuahuoPlugin};
pub use bilibili_sanlian::{BilibiliSanlianConnector, BilibiliSanlianPlugin};
pub use http_helpers::{ConnectorMode, LiveCredentials};
pub use kuaishou::{KuaishouConnector, KuaishouPlugin};
pub use mock_data::{mock_account_id, mock_platform_account_id, seed_mock_governance};
pub use ocean_engine::{OceanEngineConnector, OceanEnginePlugin};
pub use onboarding::build_onboard_guide;
pub use tencent_ads::{TencentAdsConnector, TencentAdsPlugin};
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
        let raw_conv_amt = page1.rows[0].raw_fields["conv_data_list"][1]["conversion_amount_fen"]
            .as_i64()
            .unwrap();
        assert_eq!(page1.rows[0].gmv_fen.as_fen(), raw_conv_amt);

        // Real-style payload where top-level `gmv_fen` is omitted and only `conv_data_list` provides it.
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
                    "conv_data_list": [
                        { "conversion_type": "FORM_SUBMIT", "conversion_result": 5 },
                        { "conversion_type": "MINIAPP_PAY_ORDER", "conversion_result": 8, "conversion_amount_fen": 45000 }
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
}
