//! Pluggable storage service trait and SQLite WAL storage plugin export.

pub mod migrations;
pub mod queries;
pub mod sqlite;

use adlink_domain::{
    AdAccount, AdEntity, AuditLogEntry, CaliberReconciliation, CreativeAsset,
    CreativeAttributionRow, CreativeFormat, CreativeHookType, CreativePlatformLink,
    CreativeProductionContribution, CreatorWorkloadSummary, CustomCaliberDef, DailyFundStatement,
    Organization, Person, PersonAccountBinding, PlatformId, Project, ProjectAccountLink,
    ReportQuery, SeedingBoostAttributionRow, SyncCheckpoint, SyncJobRecord, UnifiedReportRow,
};
use async_trait::async_trait;
use chrono::NaiveDate;
use serde_json::Value;
use thiserror::Error;

pub use sqlite::{open_sqlite_from_config, SqliteStorage, SqliteStoragePlugin};

pub type StorageResult<T> = Result<T, StorageError>;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("formula caliber error: {0}")]
    Formula(#[from] adlink_domain::FormulaError),
    #[error("data integrity error: {0}")]
    DataIntegrity(String),
}

/// Abstract persistence and analytical query contract registered as a kernel service.
#[async_trait]
pub trait StorageService: Send + Sync {
    async fn upsert_organization(&self, org: &Organization) -> StorageResult<()>;
    async fn list_organizations(&self) -> StorageResult<Vec<Organization>>;

    async fn upsert_person(&self, person: &Person) -> StorageResult<()>;
    async fn list_people(&self) -> StorageResult<Vec<Person>>;

    async fn upsert_ad_account(&self, acct: &AdAccount) -> StorageResult<()>;
    async fn list_ad_accounts(&self, platform: Option<PlatformId>) -> StorageResult<Vec<AdAccount>>;

    async fn upsert_person_account_binding(
        &self,
        binding: &PersonAccountBinding,
    ) -> StorageResult<()>;
    async fn list_person_account_bindings(&self) -> StorageResult<Vec<PersonAccountBinding>>;
    async fn delete_person_account_bindings_for_account(
        &self,
        ad_account_id: &str,
    ) -> StorageResult<usize>;

    async fn upsert_project(&self, project: &Project) -> StorageResult<()>;
    async fn list_projects(&self) -> StorageResult<Vec<Project>>;

    async fn upsert_project_account_link(&self, link: &ProjectAccountLink) -> StorageResult<()>;
    async fn list_project_account_links(&self) -> StorageResult<Vec<ProjectAccountLink>>;
    async fn delete_project_account_links_for_account(
        &self,
        ad_account_id: &str,
    ) -> StorageResult<usize>;

    async fn upsert_creative_asset(&self, asset: &CreativeAsset) -> StorageResult<()>;
    async fn list_creative_assets(&self) -> StorageResult<Vec<CreativeAsset>>;

    async fn upsert_creative_platform_link(&self, link: &CreativePlatformLink) -> StorageResult<()>;
    async fn list_creative_platform_links(&self) -> StorageResult<Vec<CreativePlatformLink>>;

    async fn upsert_creative_contribution(
        &self,
        contrib: &CreativeProductionContribution,
    ) -> StorageResult<()>;
    async fn list_creative_contributions(
        &self,
    ) -> StorageResult<Vec<CreativeProductionContribution>>;

    async fn upsert_custom_caliber(&self, caliber: &CustomCaliberDef) -> StorageResult<()>;
    async fn list_custom_calibers(&self) -> StorageResult<Vec<CustomCaliberDef>>;

    async fn upsert_reports(&self, rows: &[UnifiedReportRow]) -> StorageResult<usize>;
    async fn upsert_fund_statements(&self, rows: &[DailyFundStatement]) -> StorageResult<usize>;
    async fn list_fund_statements(
        &self,
        start_date: NaiveDate,
        end_date: NaiveDate,
        ad_account_id: Option<&str>,
    ) -> StorageResult<Vec<DailyFundStatement>>;

    async fn query_reports(&self, query: &ReportQuery) -> StorageResult<Vec<UnifiedReportRow>>;
    async fn query_reconciled_metrics(
        &self,
        query: &ReportQuery,
    ) -> StorageResult<(CaliberReconciliation, Vec<CaliberReconciliation>)>;

    async fn query_creative_attribution(
        &self,
        start_date: NaiveDate,
        end_date: NaiveDate,
        project_id: Option<&str>,
        format: Option<CreativeFormat>,
        hook_type: Option<CreativeHookType>,
    ) -> StorageResult<(
        Vec<CreativeAttributionRow>,
        Vec<CreatorWorkloadSummary>,
        Vec<SeedingBoostAttributionRow>,
    )>;

    async fn upsert_sync_job(&self, job: &SyncJobRecord) -> StorageResult<()>;
    async fn list_sync_jobs(&self, limit: usize) -> StorageResult<Vec<SyncJobRecord>>;

    async fn upsert_sync_checkpoint(&self, cp: &SyncCheckpoint) -> StorageResult<()>;
    async fn list_sync_checkpoints(&self, job_id: &str) -> StorageResult<Vec<SyncCheckpoint>>;

    async fn append_audit_log(&self, entry: &AuditLogEntry) -> StorageResult<()>;
    async fn list_audit_logs(&self, limit: usize) -> StorageResult<Vec<AuditLogEntry>>;

    async fn upsert_ad_entities(&self, entities: &[AdEntity]) -> StorageResult<()>;
    async fn list_ad_entities(
        &self,
        platform: Option<PlatformId>,
        ad_account_id: Option<&str>,
    ) -> StorageResult<Vec<AdEntity>>;

    async fn count_report_rows(&self) -> StorageResult<usize>;
    async fn db_health_summary(&self) -> StorageResult<Value>;

    /// Monotonically increasing epoch bumped after every committed write transaction,
    /// allowing read caches in upper layers to invalidate in O(1) without polling SQLite.
    fn write_epoch(&self) -> u64 {
        0
    }

    /// Runs `PRAGMA wal_checkpoint(TRUNCATE)` on the writer connection during graceful shutdown.
    async fn checkpoint_truncate(&self) -> StorageResult<()> {
        Ok(())
    }

    /// Reports connection pool diagnostics (reader pool count, current write epoch).
    fn pool_stats(&self) -> Value {
        serde_json::json!({})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adlink_domain::{
        AccountType, BindingRole, CreativeTaxonomy, Fen, OrgType, PersonRole, ProductionRole,
        ProjectType, QueryDimension, ReportGranularity, ReportLevel, SettlementMode, TimeLine,
    };
    use tempfile::tempdir;

    #[tokio::test]
    async fn sqlite_wal_governance_and_attribution_end_to_end() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test_adlink.db");
        let storage = SqliteStorage::open(&db_path).unwrap();

        storage
            .upsert_organization(&Organization {
                id: "org-1".into(),
                name: "Test Agency".into(),
                org_type: OrgType::Agency,
                default_currency: "CNY".into(),
            })
            .await
            .unwrap();

        storage
            .upsert_person(&Person {
                id: "p-opt-a".into(),
                org_id: "org-1".into(),
                name: "Alice".into(),
                email: None,
                department: "Performance".into(),
                role: PersonRole::Optimizer,
                monthly_cost_fen: Fen(15000_00),
                active: true,
            })
            .await
            .unwrap();

        storage
            .upsert_person(&Person {
                id: "p-opt-b".into(),
                org_id: "org-1".into(),
                name: "Bob".into(),
                email: None,
                department: "Performance".into(),
                role: PersonRole::Optimizer,
                monthly_cost_fen: Fen(15000_00),
                active: true,
            })
            .await
            .unwrap();

        storage
            .upsert_ad_account(&AdAccount {
                id: "acc-xhs".into(),
                org_id: "org-1".into(),
                platform: PlatformId::XhsJuguang,
                platform_account_id: "9001".into(),
                account_name: "XHS Main".into(),
                account_type: AccountType::AgencySub,
                rebate_rate: 0.05,
                service_fee_rate: 0.08,
                default_cash_ratio: 0.9,
                status: "active".into(),
                credential_ref: None,
                extra: Value::Null,
            })
            .await
            .unwrap();

        let d1 = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();

        // Co-optimization 60/40 split across effective date ranges.
        storage
            .upsert_person_account_binding(&PersonAccountBinding {
                id: "bind-1".into(),
                person_id: "p-opt-a".into(),
                ad_account_id: "acc-xhs".into(),
                relation_role: BindingRole::PrimaryOptimizer,
                split_weight: 0.6,
                effective_from: d1,
                effective_to: None,
            })
            .await
            .unwrap();
        storage
            .upsert_person_account_binding(&PersonAccountBinding {
                id: "bind-2".into(),
                person_id: "p-opt-b".into(),
                ad_account_id: "acc-xhs".into(),
                relation_role: BindingRole::CoOptimizer,
                split_weight: 0.4,
                effective_from: d2,
                effective_to: None,
            })
            .await
            .unwrap();

        storage
            .upsert_project(&Project {
                id: "proj-1".into(),
                org_id: "org-1".into(),
                code: "PRJ-01".into(),
                name: "Autumn Seeding".into(),
                client_name: "GlowBrand".into(),
                brand_name: "Glow".into(),
                project_type: ProjectType::SeedingContent,
                business_line: "Beauty".into(),
                settlement_mode: SettlementMode::MediaSpendPlusFee,
                budget_fen: Fen(100_000_00),
                target_roi: Some(3.0),
                target_cpa_fen: None,
                owner_person_id: Some("p-opt-a".into()),
                start_date: d1,
                end_date: None,
                tags: vec!["xhs".into()],
            })
            .await
            .unwrap();
        storage
            .upsert_project_account_link(&ProjectAccountLink {
                id: "pal-1".into(),
                project_id: "proj-1".into(),
                ad_account_id: "acc-xhs".into(),
                campaign_filter_regex: None,
                allocation_weight: 1.0,
            })
            .await
            .unwrap();

        storage
            .upsert_creative_asset(&CreativeAsset {
                id: "cr-1".into(),
                project_id: "proj-1".into(),
                asset_code: "CR-001".into(),
                title: "Morning Serum Unboxing".into(),
                taxonomy: CreativeTaxonomy {
                    format: CreativeFormat::ImageTextNote,
                    hook_type: CreativeHookType::Unboxing,
                    scene_tag: "morning_routine".into(),
                    selling_point: "niacinamide".into(),
                    tone: "authentic".into(),
                    custom_tags: Default::default(),
                },
                production_cost_fen: Fen(500_00),
                production_hours: 10.0,
                created_date: d1,
                status: "active".into(),
            })
            .await
            .unwrap();
        storage
            .upsert_creative_platform_link(&CreativePlatformLink {
                id: "cpl-1".into(),
                asset_id: "cr-1".into(),
                platform: PlatformId::XhsJuguang,
                ad_account_id: "acc-xhs".into(),
                platform_creative_id: "cre_999".into(),
                platform_content_id: Some("note_abc".into()),
            })
            .await
            .unwrap();
        storage
            .upsert_creative_contribution(&CreativeProductionContribution {
                id: "cpc-1".into(),
                asset_id: "cr-1".into(),
                person_id: "p-opt-a".into(),
                role: ProductionRole::Scriptwriter,
                work_hours: 10.0,
                credit_weight: 1.0,
                piece_cost_fen: Fen(500_00),
            })
            .await
            .unwrap();

        let row = UnifiedReportRow {
            platform: PlatformId::XhsJuguang,
            ad_account_id: "acc-xhs".into(),
            platform_account_id: "9001".into(),
            stat_date: d2,
            stat_hour: -1,
            granularity: ReportGranularity::Daily,
            level: ReportLevel::Creative,
            entity_id: "cre_999".into(),
            entity_name: "Morning Serum Ad".into(),
            parent_entity_id: Some("camp_1".into()),
            content_id: Some("note_abc".into()),
            time_line: TimeLine::ReportingTime,
            gross_cost_fen: Fen(1000_00),
            cash_cost_fen: Fen(900_00),
            grant_cost_fen: Fen(100_00),
            kol_order_fee_fen: Fen::ZERO,
            platform_service_fee_fen: Fen::ZERO,
            impressions: 50_000,
            clicks: 2_500,
            conversions: 100,
            deep_conversions: 20,
            leads: 0,
            orders: 80,
            gmv_fen: Fen(4000_00),
            interactions: 1200,
            video_plays: 0,
            effective_plays: 0,
            new_follows: 150,
            store_verifies: 0,
            raw_fields: Default::default(),
        };

        // Upsert twice to verify idempotency.
        storage.upsert_reports(&[row.clone()]).await.unwrap();
        storage.upsert_reports(&[row.clone()]).await.unwrap();
        assert_eq!(storage.count_report_rows().await.unwrap(), 1);

        // Also insert an Account-level row for the same account & date; verify default queries
        // do NOT double-count Account + Creative together when `levels` is empty!
        let acct_row = UnifiedReportRow {
            level: ReportLevel::Account,
            entity_id: "9001".into(),
            entity_name: "XHS Main".into(),
            content_id: None,
            ..row
        };
        storage.upsert_reports(&[acct_row]).await.unwrap();
        assert_eq!(storage.count_report_rows().await.unwrap(), 2);

        let (summary, by_person) = storage
            .query_reconciled_metrics(&ReportQuery {
                start_date: d1,
                end_date: d2,
                group_by: vec![QueryDimension::Person],
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(summary.gross_media_cost_yuan, 1000.0);
        assert_eq!(by_person.len(), 2);

        let (creative_only, _) = storage
            .query_reconciled_metrics(&ReportQuery {
                start_date: d1,
                end_date: d2,
                levels: vec![ReportLevel::Creative],
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(creative_only.gross_media_cost_yuan, 1000.0);

        let (creatives, creators, _seeding) = storage
            .query_creative_attribution(d1, d2, None, None, None)
            .await
            .unwrap();
        assert_eq!(creatives.len(), 1);
        assert_eq!(creatives[0].gmv_per_hour_yuan, 400.0);
        assert_eq!(creators.len(), 1);
        assert_eq!(creators[0].total_work_hours, 10.0);
    }

    #[tokio::test]
    async fn daily_hourly_dedup_fund_reconciliation_and_campaign_regex_work() {
        let dir = tempdir().unwrap();
        let storage = SqliteStorage::open(dir.path().join("reconcile.db")).unwrap();
        let d = NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();

        storage
            .upsert_organization(&Organization {
                id: "org-1".into(),
                name: "Agency".into(),
                org_type: OrgType::Agency,
                default_currency: "CNY".into(),
            })
            .await
            .unwrap();
        storage
            .upsert_ad_account(&AdAccount {
                id: "acc-oe".into(),
                org_id: "org-1".into(),
                platform: PlatformId::OceanEngineAd,
                platform_account_id: "1789".into(),
                account_name: "OE Main".into(),
                account_type: AccountType::AgencySub,
                rebate_rate: 0.05,
                service_fee_rate: 0.07,
                default_cash_ratio: 0.90,
                status: "active".into(),
                credential_ref: None,
                extra: Value::Null,
            })
            .await
            .unwrap();

        // Two projects sharing the same ad account, separated by campaign_filter_regex!
        storage
            .upsert_project(&Project {
                id: "prj-brand".into(),
                org_id: "org-1".into(),
                code: "B1".into(),
                name: "Brand Project".into(),
                client_name: "C".into(),
                brand_name: "B".into(),
                project_type: ProjectType::BrandAwareness,
                business_line: "Beauty".into(),
                settlement_mode: SettlementMode::MediaSpendPlusFee,
                budget_fen: Fen(100_000_00),
                target_roi: None,
                target_cpa_fen: None,
                owner_person_id: None,
                start_date: d,
                end_date: None,
                tags: vec![],
            })
            .await
            .unwrap();
        storage
            .upsert_project(&Project {
                id: "prj-perf".into(),
                org_id: "org-1".into(),
                code: "P1".into(),
                name: "Perf Project".into(),
                client_name: "C".into(),
                brand_name: "B".into(),
                project_type: ProjectType::EcommerceGmv,
                business_line: "Beauty".into(),
                settlement_mode: SettlementMode::MediaSpendPlusFee,
                budget_fen: Fen(100_000_00),
                target_roi: None,
                target_cpa_fen: None,
                owner_person_id: None,
                start_date: d,
                end_date: None,
                tags: vec![],
            })
            .await
            .unwrap();
        storage
            .upsert_project_account_link(&ProjectAccountLink {
                id: "pal-b".into(),
                project_id: "prj-brand".into(),
                ad_account_id: "acc-oe".into(),
                campaign_filter_regex: Some("(?i)brand".into()),
                allocation_weight: 1.0,
            })
            .await
            .unwrap();
        storage
            .upsert_project_account_link(&ProjectAccountLink {
                id: "pal-p".into(),
                project_id: "prj-perf".into(),
                ad_account_id: "acc-oe".into(),
                campaign_filter_regex: Some("(?i)perf".into()),
                allocation_weight: 1.0,
            })
            .await
            .unwrap();

        // Insert Daily + Hourly + RequestTime campaign rows for the same day.
        let daily_brand = UnifiedReportRow {
            platform: PlatformId::OceanEngineAd,
            ad_account_id: "acc-oe".into(),
            platform_account_id: "1789".into(),
            stat_date: d,
            stat_hour: -1,
            granularity: ReportGranularity::Daily,
            level: ReportLevel::Campaign,
            entity_id: "c_brand".into(),
            entity_name: "Q4 Brand Push".into(),
            time_line: TimeLine::ReportingTime,
            gross_cost_fen: Fen(1000_00),
            cash_cost_fen: Fen(900_00),
            grant_cost_fen: Fen(100_00),
            ..Default::default()
        };
        let hourly_brand = UnifiedReportRow {
            stat_hour: 10,
            granularity: ReportGranularity::Hourly,
            gross_cost_fen: Fen(400_00),
            cash_cost_fen: Fen(360_00),
            grant_cost_fen: Fen(40_00),
            ..daily_brand.clone()
        };
        let daily_perf = UnifiedReportRow {
            entity_id: "c_perf".into(),
            entity_name: "Q4 Perf ROAS".into(),
            gross_cost_fen: Fen(2000_00),
            cash_cost_fen: Fen(1800_00),
            grant_cost_fen: Fen(200_00),
            ..daily_brand.clone()
        };
        let request_time_perf = UnifiedReportRow {
            time_line: TimeLine::RequestTime,
            ..daily_perf.clone()
        };
        storage
            .upsert_reports(&[daily_brand, hourly_brand, daily_perf, request_time_perf])
            .await
            .unwrap();

        // Insert an actual DailyFundStatement showing 70% cash / 30% grant (overriding 90% estimate!).
        storage
            .upsert_fund_statements(&[DailyFundStatement {
                platform: PlatformId::OceanEngineAd,
                ad_account_id: "acc-oe".into(),
                platform_account_id: "1789".into(),
                stat_date: d,
                cash_balance_fen: Fen(50_000_00),
                grant_balance_fen: Fen(20_000_00),
                cash_cost_fen: Fen(2100_00),
                grant_cost_fen: Fen(900_00),
                shared_wallet_cost_fen: Fen::ZERO,
                income_fen: Fen::ZERO,
                raw_fields: Default::default(),
            }])
            .await
            .unwrap();

        let (tot, by_proj) = storage
            .query_reconciled_metrics(&ReportQuery {
                start_date: d,
                end_date: d,
                group_by: vec![QueryDimension::Project],
                ..Default::default()
            })
            .await
            .unwrap();

        // Daily+Hourly & Reporting+Request deduplicated cleanly: 1000 + 2000 = 3000 yuan!
        assert_eq!(tot.gross_media_cost_yuan, 3000.0);
        // Fund statement 70/30 ratio reconciled over 90% static fallback: 2100 cash / 900 grant!
        assert_eq!(tot.cash_cost_yuan, 2100.0);
        assert_eq!(tot.grant_cost_yuan, 900.0);
        assert_eq!(by_proj.len(), 2);
        let brand_row = by_proj
            .iter()
            .find(|g| g.label.contains("Brand Project"))
            .unwrap();
        assert_eq!(brand_row.gross_media_cost_yuan, 1000.0);
        let perf_row = by_proj
            .iter()
            .find(|g| g.label.contains("Perf Project"))
            .unwrap();
        assert_eq!(perf_row.gross_media_cost_yuan, 2000.0);
    }

    #[tokio::test]
    async fn sqlite_wal_reader_pool_handles_concurrent_readers_and_writer() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("pooled.db");
        let storage = std::sync::Arc::new(SqliteStorage::open_with_pool(&db_path, 6).unwrap());
        let initial_epoch = storage.write_epoch();
        assert!(initial_epoch >= 1);

        storage
            .upsert_organization(&Organization {
                id: "org-pool".into(),
                name: "Pool Org".into(),
                org_type: OrgType::Agency,
                default_currency: "CNY".into(),
            })
            .await
            .unwrap();
        assert!(storage.write_epoch() > initial_epoch);

        storage
            .upsert_ad_account(&AdAccount {
                id: "acc-pool".into(),
                org_id: "org-pool".into(),
                platform: PlatformId::MetaAds,
                platform_account_id: "act_1".into(),
                account_name: "Pool Meta".into(),
                account_type: AccountType::AgencySub,
                rebate_rate: 0.02,
                service_fee_rate: 0.07,
                default_cash_ratio: 1.0,
                status: "active".into(),
                credential_ref: None,
                extra: Value::Null,
            })
            .await
            .unwrap();

        let d = NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();
        let writer_storage = std::sync::Arc::clone(&storage);
        let writer_handle = tokio::spawn(async move {
            for i in 0..5 {
                let row = UnifiedReportRow {
                    platform: PlatformId::MetaAds,
                    ad_account_id: "acc-pool".into(),
                    platform_account_id: "act_1".into(),
                    stat_date: d,
                    stat_hour: -1,
                    granularity: ReportGranularity::Daily,
                    level: ReportLevel::Campaign,
                    entity_id: format!("c_{i}"),
                    entity_name: format!("Camp {i}"),
                    parent_entity_id: None,
                    content_id: None,
                    time_line: TimeLine::ReportingTime,
                    gross_cost_fen: Fen(100_00),
                    cash_cost_fen: Fen(100_00),
                    grant_cost_fen: Fen::ZERO,
                    kol_order_fee_fen: Fen::ZERO,
                    platform_service_fee_fen: Fen::ZERO,
                    impressions: 1000,
                    clicks: 50,
                    conversions: 5,
                    deep_conversions: 0,
                    leads: 0,
                    orders: 5,
                    gmv_fen: Fen(400_00),
                    interactions: 10,
                    video_plays: 100,
                    effective_plays: 50,
                    new_follows: 0,
                    store_verifies: 0,
                    raw_fields: Default::default(),
                };
                writer_storage.upsert_reports(&[row]).await.unwrap();
            }
        });

        let mut reader_handles = Vec::new();
        for _ in 0..16 {
            let s = std::sync::Arc::clone(&storage);
            reader_handles.push(tokio::spawn(async move {
                let _ = s.list_ad_accounts(None).await.unwrap();
                let _ = s
                    .query_reconciled_metrics(&ReportQuery {
                        start_date: d,
                        end_date: d,
                        group_by: vec![QueryDimension::Platform],
                        ..Default::default()
                    })
                    .await
                    .unwrap();
                let summary = s.db_health_summary().await.unwrap();
                assert_eq!(summary["reader_pool_size"], 6);
            }));
        }

        writer_handle.await.unwrap();
        for h in reader_handles {
            h.await.unwrap();
        }
        storage.checkpoint_truncate().await.unwrap();
        let rows = storage.count_report_rows().await.unwrap();
        assert_eq!(rows, 5);
    }
}
