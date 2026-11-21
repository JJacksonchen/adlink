//! High-concurrency sync engine, date chunking, hierarchical rate limiting, and connector trait.

pub mod chunker;
pub mod connector;
pub mod ratelimit;
pub mod retry;
pub mod sync;

pub use chunker::{ChunkerError, DateRangeChunker};
pub use connector::{
    AdPlatformConnector, ConnectorError, ConnectorResult, CredentialFieldSpec,
    PlatformOnboardGuide, ReportFetchPage, ReportFetchRequest,
};
pub use ratelimit::{RateLimiterPool, TokenBucket};
pub use retry::RetryPolicy;
pub use sync::{PlatformSyncStat, SyncCoordinator, SyncError, SyncJobSummary, SyncRequest};

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use adlink_domain::{
        AccountType, AdAccount, AdEntity, AdMutationRequest, AdMutationResult, DailyFundStatement,
        Fen, OrgType, Organization, PlatformId, ReportGranularity, ReportLevel, UnifiedReportRow,
    };
    use adlink_kernel::{Kernel, Plugin, PluginCategory, PluginContext, PluginManifest};
    use adlink_storage::{SqliteStorage, SqliteStoragePlugin, StorageService};
    use async_trait::async_trait;
    use chrono::NaiveDate;
    use serde_json::Value;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn chunker_handles_inclusive_boundaries_and_leap_years() {
        let s = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let e = NaiveDate::from_ymd_opt(2024, 12, 31).unwrap();
        let chunks_90 = DateRangeChunker::split(s, e, 90).unwrap();
        // 2024 is leap year (366 days): 90*4 + 6 => 5 chunks
        assert_eq!(chunks_90.len(), 5);
        assert_eq!(chunks_90.first().unwrap().0, s);
        assert_eq!(chunks_90.last().unwrap().1, e);
        for w in chunks_90.windows(2) {
            assert_eq!(w[0].1.succ_opt().unwrap(), w[1].0);
        }

        let chunks_7 = DateRangeChunker::split(s, NaiveDate::from_ymd_opt(2024, 1, 14).unwrap(), 7).unwrap();
        assert_eq!(chunks_7.len(), 2);
        assert_eq!(chunks_7[0].1, NaiveDate::from_ymd_opt(2024, 1, 7).unwrap());
        assert_eq!(chunks_7[1].0, NaiveDate::from_ymd_opt(2024, 1, 8).unwrap());
    }

    #[tokio::test]
    async fn retry_policy_retries_transient_then_succeeds() {
        let policy = RetryPolicy {
            max_attempts: 3,
            base_delay_ms: 5,
            max_delay_ms: 20,
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_ref = Arc::clone(&calls);
        let res = policy
            .execute(|| {
                let c = Arc::clone(&calls_ref);
                async move {
                    let n = c.fetch_add(1, Ordering::SeqCst);
                    if n < 2 {
                        Err(ConnectorError::RateLimited {
                            platform: PlatformId::AlipayDenghuo,
                            retry_after_ms: Some(5),
                            message: "429".into(),
                        })
                    } else {
                        Ok(42)
                    }
                }
            })
            .await
            .unwrap();
        assert_eq!(res, 42);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    struct MockTestConnector {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl AdPlatformConnector for MockTestConnector {
        fn platform_id(&self) -> PlatformId {
            PlatformId::AlipayDenghuo
        }
        fn mode_label(&self) -> &'static str {
            "mock"
        }
        fn onboard_guide(&self) -> PlatformOnboardGuide {
            PlatformOnboardGuide {
                platform: PlatformId::AlipayDenghuo,
                mode: "mock".into(),
                developer_portal_url: "https://open.alipay.com".into(),
                oauth_or_auth_flow: "RSA2".into(),
                optimal_login_strategy: "official_mcp_bridge".into(),
                fallback_login_strategy: "direct_api_account_discovery".into(),
                ad_creation_support: adlink_domain::AdCreationSupport::NotSupportedPublicly,
                ad_creation_ask_prompt: "ask user before creation".into(),
                ad_creation_risks: vec![],
                required_permissions: vec![],
                prerequisite_steps: vec![],
                credential_fields: vec![],
                toml_snippet: "".into(),
                verification_steps: vec![],
                gotchas: vec![],
            }
        }
        async fn fetch_reports_page(&self, req: &ReportFetchRequest) -> ConnectorResult<ReportFetchPage> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut rows = Vec::new();
            let mut cur = req.start_date;
            while cur <= req.end_date {
                rows.push(UnifiedReportRow {
                    platform: PlatformId::AlipayDenghuo,
                    ad_account_id: req.account.id.clone(),
                    platform_account_id: req.account.platform_account_id.clone(),
                    stat_date: cur,
                    level: req.level,
                    entity_id: format!("ent_{}", req.level.as_str()),
                    entity_name: "Demo".into(),
                    gross_cost_fen: Fen(5000),
                    cash_cost_fen: Fen(4500),
                    grant_cost_fen: Fen(500),
                    impressions: 1000,
                    clicks: 50,
                    ..Default::default()
                });
                cur = cur.succ_opt().unwrap();
            }
            Ok(ReportFetchPage {
                rows,
                has_more: false,
                total_count: None,
                next_cursor: None,
            })
        }
        async fn fetch_fund_statements(
            &self,
            _account: &AdAccount,
            _start_date: NaiveDate,
            _end_date: NaiveDate,
        ) -> ConnectorResult<Vec<DailyFundStatement>> {
            Ok(vec![])
        }
        async fn list_ad_entities(
            &self,
            _account: &AdAccount,
            _level: ReportLevel,
        ) -> ConnectorResult<Vec<AdEntity>> {
            Ok(vec![])
        }
        async fn mutate_ad_entities(
            &self,
            _account: &AdAccount,
            req: &AdMutationRequest,
        ) -> ConnectorResult<AdMutationResult> {
            Ok(AdMutationResult {
                dry_run: req.dry_run,
                accepted: true,
                platform: req.platform,
                ad_account_id: req.ad_account_id.clone(),
                affected_entity_ids: req.entity_ids.clone(),
                summary: "ok".into(),
                guardrail_notes: vec![],
                platform_response: Value::Null,
            })
        }
    }

    struct MockTestPlugin(Arc<MockTestConnector>);

    #[async_trait]
    impl Plugin for MockTestPlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: "connector.alipay_denghuo".into(),
                name: "Mock Alipay".into(),
                version: "0.1.0".into(),
                description: "test".into(),
                category: PluginCategory::Connector,
                provides: vec!["connector:alipay_denghuo".into()],
                requires: vec!["storage".into()],
                config_schema: Value::Null,
            }
        }
        async fn mount(&self, ctx: &mut PluginContext) -> adlink_kernel::KernelResult<()> {
            let svc: Arc<dyn AdPlatformConnector> =
                Arc::clone(&self.0) as Arc<dyn AdPlatformConnector>;
            ctx.provide_named_service("alipay_denghuo", svc);
            Ok(())
        }
    }

    #[tokio::test]
    async fn sync_coordinator_chunks_and_resumes_idempotently() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(SqliteStorage::open(dir.path().join("sync.db")).unwrap());
        storage
            .upsert_organization(&Organization {
                id: "org-1".into(),
                name: "Org".into(),
                org_type: OrgType::Agency,
                default_currency: "CNY".into(),
            })
            .await
            .unwrap();
        storage
            .upsert_ad_account(&AdAccount {
                id: "acc-ali".into(),
                org_id: "org-1".into(),
                platform: PlatformId::AlipayDenghuo,
                platform_account_id: "20881234".into(),
                account_name: "Alipay Account".into(),
                account_type: AccountType::AgencySub,
                rebate_rate: 0.05,
                service_fee_rate: 0.05,
                default_cash_ratio: 0.9,
                status: "active".into(),
                credential_ref: None,
                extra: Value::Null,
            })
            .await
            .unwrap();

        let kernel = Arc::new(Kernel::new());
        kernel
            .mount(Arc::new(SqliteStoragePlugin::new(Arc::clone(&storage))), Value::Null)
            .await
            .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let conn = Arc::new(MockTestConnector {
            calls: Arc::clone(&calls),
        });
        kernel.mount(Arc::new(MockTestPlugin(conn)), Value::Null).await.unwrap();

        let coord = SyncCoordinator::new(Arc::clone(&kernel));
        let start = NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 9, 21).unwrap();
        let summary = coord
            .sync_reports(SyncRequest {
                platforms: vec![PlatformId::AlipayDenghuo],
                ad_account_ids: vec![],
                start_date: start,
                end_date: end,
                granularity: ReportGranularity::Daily,
                levels: vec![ReportLevel::Account],
                time_line: Default::default(),
                max_concurrency: 4,
                page_size: 100,
                resume_job_id: None,
                qps_override: None,
            })
            .await
            .unwrap();

        // 21 days / max 7 days per chunk = 3 chunks -> 21 rows
        assert_eq!(summary.total_chunks, 3);
        assert_eq!(summary.completed_chunks, 3);
        assert_eq!(summary.rows_upserted, 21);
        assert_eq!(calls.load(Ordering::SeqCst), 3);

        // Resuming the completed job must skip all 3 chunks cleanly (0 new connector calls).
        let resumed = coord
            .sync_reports(SyncRequest {
                platforms: vec![PlatformId::AlipayDenghuo],
                ad_account_ids: vec![],
                start_date: start,
                end_date: end,
                granularity: ReportGranularity::Daily,
                levels: vec![ReportLevel::Account],
                time_line: Default::default(),
                max_concurrency: 4,
                page_size: 100,
                resume_job_id: Some(summary.job_id),
                qps_override: None,
            })
            .await
            .unwrap();
        assert_eq!(resumed.skipped_resumed_chunks, 3);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn after_normalize_row_hook_enriches_and_filters_rows() {
        use adlink_domain::ReportQuery;
        use adlink_kernel::{HookDecision, HookHandler, HookPoint};

        struct EnrichAndFilterHook;

        #[async_trait]
        impl HookHandler for EnrichAndFilterHook {
            fn id(&self) -> &str {
                "test.enrich_filter"
            }
            async fn handle(&self, _point: HookPoint, mut payload: Value) -> HookDecision {
                if payload.get("stat_date").and_then(Value::as_str) == Some("2026-09-02") {
                    return HookDecision::Reject {
                        code: "FILTER_DAY".into(),
                        reason: "skip test day".into(),
                    };
                }
                if let Some(raw) = payload.get_mut("raw_fields").and_then(Value::as_object_mut) {
                    raw.insert("enriched_tag".into(), Value::String("ok".into()));
                }
                HookDecision::Modified(payload)
            }
        }

        struct EnrichPlugin;

        #[async_trait]
        impl Plugin for EnrichPlugin {
            fn manifest(&self) -> PluginManifest {
                PluginManifest {
                    id: "hook.enrich_filter".into(),
                    name: "Enrich Filter".into(),
                    version: "0.1.0".into(),
                    description: "test".into(),
                    category: PluginCategory::Custom("enrich".into()),
                    provides: vec![],
                    requires: vec![],
                    config_schema: Value::Null,
                }
            }
            async fn mount(&self, ctx: &mut PluginContext) -> adlink_kernel::KernelResult<()> {
                ctx.register_hook(HookPoint::AfterNormalizeRow, Arc::new(EnrichAndFilterHook));
                Ok(())
            }
        }

        let dir = tempdir().unwrap();
        let storage = Arc::new(SqliteStorage::open(dir.path().join("hook_sync.db")).unwrap());
        storage
            .upsert_organization(&Organization {
                id: "org-1".into(),
                name: "Org".into(),
                org_type: OrgType::Agency,
                default_currency: "CNY".into(),
            })
            .await
            .unwrap();
        storage
            .upsert_ad_account(&AdAccount {
                id: "acc-ali".into(),
                org_id: "org-1".into(),
                platform: PlatformId::AlipayDenghuo,
                platform_account_id: "20881234".into(),
                account_name: "Alipay Account".into(),
                account_type: AccountType::AgencySub,
                rebate_rate: 0.05,
                service_fee_rate: 0.05,
                default_cash_ratio: 0.9,
                status: "active".into(),
                credential_ref: None,
                extra: Value::Null,
            })
            .await
            .unwrap();

        let kernel = Arc::new(Kernel::new());
        kernel
            .mount(Arc::new(SqliteStoragePlugin::new(Arc::clone(&storage))), Value::Null)
            .await
            .unwrap();
        let conn = Arc::new(MockTestConnector {
            calls: Arc::new(AtomicUsize::new(0)),
        });
        kernel.mount(Arc::new(MockTestPlugin(conn)), Value::Null).await.unwrap();
        kernel.mount(Arc::new(EnrichPlugin), Value::Null).await.unwrap();

        let coord = SyncCoordinator::new(Arc::clone(&kernel));
        let start = NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 9, 3).unwrap();
        let summary = coord
            .sync_reports(SyncRequest {
                platforms: vec![PlatformId::AlipayDenghuo],
                ad_account_ids: vec![],
                start_date: start,
                end_date: end,
                granularity: ReportGranularity::Daily,
                levels: vec![ReportLevel::Account],
                time_line: Default::default(),
                max_concurrency: 2,
                page_size: 100,
                resume_job_id: None,
                qps_override: None,
            })
            .await
            .unwrap();

        // 3 days minus 1 filtered day (2026-09-02) = 2 rows upserted, each enriched with raw_fields["enriched_tag"]
        assert_eq!(summary.rows_upserted, 2);
        let stored = storage
            .query_reports(&ReportQuery {
                start_date: start,
                end_date: end,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(stored.len(), 2);
        assert!(stored
            .iter()
            .all(|r| r.raw_fields.get("enriched_tag") == Some(&Value::String("ok".into()))));
    }
}
