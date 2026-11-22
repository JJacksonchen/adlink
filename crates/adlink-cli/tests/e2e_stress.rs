//! Commercial-grade 180-day multi-platform high-concurrency batch sync, caliber reconciliation,
//! creative attribution, and MCP protocol stress tests.

use std::sync::Arc;

use adlink_connectors::{mount_all_connectors, seed_mock_governance, ConnectorSuiteConfig};
use adlink_domain::{PlatformId, QueryDimension, ReportGranularity, ReportLevel, ReportQuery};
use adlink_engine::{SyncCoordinator, SyncRequest};
use adlink_kernel::Kernel;
use adlink_mcp::{AdSafetyGuardrailPlugin, GuardrailConfig, McpHandler};
use adlink_storage::{SqliteStorage, SqliteStoragePlugin, StorageService};
use chrono::NaiveDate;
use serde_json::{json, Value};
use tempfile::tempdir;

#[tokio::test]
async fn stress_test_180_day_ten_platform_concurrent_sync_and_attribution() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("stress_180d.db");
    let storage = Arc::new(SqliteStorage::open(&db_path).unwrap());
    let kernel = Arc::new(Kernel::new());

    kernel
        .mount(
            Arc::new(SqliteStoragePlugin::new(Arc::clone(&storage))),
            Value::Null,
        )
        .await
        .unwrap();
    kernel
        .mount(
            Arc::new(AdSafetyGuardrailPlugin::new(
                GuardrailConfig::default(),
                adlink_domain::AdCreationConfig::default(),
            )),
            Value::Null,
        )
        .await
        .unwrap();
    mount_all_connectors(&kernel, &ConnectorSuiteConfig::default())
        .await
        .unwrap();
    seed_mock_governance(storage.as_ref()).await.unwrap();

    // 180-day range: 2026-05-01 ..= 2026-10-27
    let start = NaiveDate::from_ymd_opt(2026, 5, 1).unwrap();
    let end = NaiveDate::from_ymd_opt(2026, 10, 27).unwrap();

    let coord = SyncCoordinator::new(Arc::clone(&kernel));
    let summary = coord
        .sync_reports(SyncRequest {
            platforms: PlatformId::ALL.to_vec(),
            ad_account_ids: vec![],
            start_date: start,
            end_date: end,
            granularity: ReportGranularity::Daily,
            levels: vec![
                ReportLevel::Account,
                ReportLevel::Campaign,
                ReportLevel::Creative,
                ReportLevel::ContentOrder,
            ],
            time_line: Default::default(),
            max_concurrency: 16,
            page_size: 200,
            resume_job_id: None,
            qps_override: Some(500.0),
        })
        .await
        .unwrap();

    assert_eq!(summary.status, "completed");
    assert_eq!(summary.failed_chunks, 0);
    assert_eq!(summary.by_platform.len(), 15);
    assert!(
        summary.rows_upserted >= 10_000,
        "expected >= 10000 rows for 180d x 15 platforms x 4 levels, got {}",
        summary.rows_upserted
    );

    let count_first = storage.count_report_rows().await.unwrap();
    assert_eq!(count_first, summary.rows_upserted);

    // Re-sync idempotency check: upserting exact same window must not duplicate any row.
    let summary2 = coord
        .sync_reports(SyncRequest {
            platforms: vec![PlatformId::TencentAds, PlatformId::AlipayDenghuo],
            ad_account_ids: vec![],
            start_date: start,
            end_date: end,
            granularity: ReportGranularity::Daily,
            levels: vec![ReportLevel::Creative],
            time_line: Default::default(),
            max_concurrency: 8,
            page_size: 200,
            resume_job_id: None,
            qps_override: Some(500.0),
        })
        .await
        .unwrap();
    assert_eq!(summary2.failed_chunks, 0);
    assert_eq!(storage.count_report_rows().await.unwrap(), count_first);

    // Verify caliber reconciliation math across all platforms & custom formulas.
    let (total, by_plat) = storage
        .query_reconciled_metrics(&ReportQuery {
            start_date: start,
            end_date: end,
            levels: vec![ReportLevel::Creative, ReportLevel::ContentOrder],
            group_by: vec![QueryDimension::Platform],
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(by_plat.len(), 15);
    for overseas in ["tiktok_ads", "meta_ads", "google_ads", "apple_search_ads", "amazon_ads"] {
        assert!(
            by_plat.iter().any(|g| g.label == overseas && g.gross_media_cost_yuan > 0.0),
            "missing reconciled overseas platform {overseas}"
        );
    }
    assert!(total.gross_media_cost_yuan > 0.0);
    assert!(total.cash_cost_yuan > 0.0);
    assert!(total.full_funnel_cost_yuan > total.gross_media_cost_yuan);
    assert!(total.custom_metrics.contains_key("effective_net_roi"));
    assert!(total.custom_metrics.contains_key("agency_gross_profit_yuan"));

    // Verify Person-Account effective-dated split attribution (`per-opt-lin` vs `per-opt-zhou`).
    let (_total_p, by_person) = storage
        .query_reconciled_metrics(&ReportQuery {
            start_date: start,
            end_date: end,
            levels: vec![ReportLevel::Account],
            group_by: vec![QueryDimension::Person],
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(by_person.len() >= 4);

    // Verify Creative Taxonomy + Creator Workload Yield + Cross-Platform Seeding+Boost (note_id/bvid).
    let (creatives, creators, seeding) = storage
        .query_creative_attribution(start, end, None, None, None)
        .await
        .unwrap();
    assert_eq!(creatives.len(), 12);
    assert_eq!(creators.len(), 4);
    assert!(creators.iter().all(|c| c.gmv_per_work_hour_yuan > 0.0));
    assert!(seeding.iter().any(|s| s.content_id == "note_xhs_101" && s.boost_platform.is_some()));
    assert!(seeding.iter().any(|s| s.content_id == "BV1xx411c7mD" && s.boost_platform.is_some()));

    // Verify MCP handler queries cleanly over the 180-day seeded database.
    let mcp = McpHandler::new(Arc::clone(&kernel), None);
    let mcp_resp = mcp
        .handle_raw_json(
            &json!({
                "jsonrpc": "2.0",
                "id": 99,
                "method": "tools/call",
                "params": {
                    "name": "adlink_reconcile_calibers",
                    "arguments": {
                        "start_date": start.to_string(),
                        "end_date": end.to_string()
                    }
                }
            })
            .to_string(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(mcp_resp.result.unwrap()["isError"], false);
}
