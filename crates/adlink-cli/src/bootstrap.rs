//! Microkernel plugin bootstrapping and optional realistic mock dataset initialization.

use std::sync::Arc;

use adlink_connectors::{mount_all_connectors, seed_mock_governance};
use adlink_domain::{FxRateTable, PlatformId, ReportLevel};
use adlink_engine::{SyncCoordinator, SyncRequest};
use adlink_kernel::Kernel;
use adlink_mcp::AdSafetyGuardrailPlugin;
use adlink_storage::{SqliteStorage, SqliteStoragePlugin, StorageService};
use chrono::{Duration, Utc};
use serde_json::Value;

use crate::config::AdLinkConfig;

pub struct BootstrappedRuntime {
    pub kernel: Arc<Kernel>,
    pub storage: Arc<SqliteStorage>,
    pub fx_rates: Arc<FxRateTable>,
}

/// Boots the Cordis-inspired microkernel and mounts storage WAL pool, FX table, safety guardrail, and all 15 connectors.
pub async fn bootstrap_kernel(cfg: &AdLinkConfig) -> anyhow::Result<BootstrappedRuntime> {
    let storage = Arc::new(SqliteStorage::open_with_pool(
        &cfg.storage.db_path,
        cfg.storage.reader_pool_size,
    )?);
    let fx_rates = Arc::new(FxRateTable::with_overrides(cfg.fx_rates.clone()));
    let kernel = Arc::new(Kernel::new());

    kernel
        .mount(
            Arc::new(SqliteStoragePlugin::new(Arc::clone(&storage))),
            serde_json::json!({
                "db_path": cfg.storage.db_path,
                "reader_pool_size": cfg.storage.reader_pool_size
            }),
        )
        .await?;

    kernel
        .mount(
            Arc::new(AdSafetyGuardrailPlugin::new(
                cfg.guardrail.clone(),
                cfg.ad_creation.clone(),
            )),
            serde_json::to_value(&cfg.guardrail)?,
        )
        .await?;

    mount_all_connectors(&kernel, &cfg.connector_suite()).await?;

    Ok(BootstrappedRuntime {
        kernel,
        storage,
        fx_rates,
    })
}

/// Seeds realistic organization, `人-户关系`, `项目类型分类`, `创意类型分类` + workload credits,
/// and triggers a multi-platform mock sync across the trailing `days` window.
pub async fn seed_mock_all(
    runtime: &BootstrappedRuntime,
    days: i64,
    concurrency: usize,
) -> anyhow::Result<usize> {
    seed_mock_governance(runtime.storage.as_ref()).await?;

    let end = Utc::now().date_naive();
    let start = end - Duration::days((days - 1).max(0));

    let coord = SyncCoordinator::new(Arc::clone(&runtime.kernel));
    let summary = coord
        .sync_reports(SyncRequest {
            platforms: PlatformId::ALL.to_vec(),
            ad_account_ids: vec![],
            start_date: start,
            end_date: end,
            granularity: adlink_domain::ReportGranularity::Daily,
            levels: vec![
                ReportLevel::Account,
                ReportLevel::Campaign,
                ReportLevel::Creative,
                ReportLevel::ContentOrder,
            ],
            time_line: Default::default(),
            max_concurrency: concurrency,
            page_size: 200,
            resume_job_id: None,
            qps_override: None,
        })
        .await?;

    let _ = coord
        .sync_fund_statements(PlatformId::ALL.to_vec(), vec![], start, end)
        .await?;

    let total_rows = runtime.storage.count_report_rows().await?;
    tracing::info!(
        job_id = %summary.job_id,
        upserted = summary.rows_upserted,
        total_rows,
        elapsed_ms = summary.elapsed_ms,
        throughput_rps = summary.throughput_rows_per_sec,
        "seeded realistic multi-platform mock dataset"
    );
    Ok(total_rows)
}

pub fn kernel_plugin_summary(plugins: &[adlink_kernel::PluginStatus]) -> Value {
    serde_json::json!({
        "count": plugins.len(),
        "plugins": plugins
    })
}
