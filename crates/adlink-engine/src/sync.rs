//! High-concurrency batch sync coordinator with checkpoint resume and kernel hook integration.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use adlink_domain::{
    AdAccount, PlatformId, ReportGranularity, ReportLevel, SyncCheckpoint, SyncJobRecord, TimeLine,
    UnifiedReportRow,
};
use adlink_kernel::{HookPoint, Kernel, KernelEvent};
use adlink_storage::StorageService;
use chrono::{NaiveDate, Utc};
use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::chunker::{ChunkerError, DateRangeChunker};
use crate::connector::{AdPlatformConnector, ReportFetchRequest};
use crate::ratelimit::RateLimiterPool;
use crate::retry::RetryPolicy;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("storage error: {0}")]
    Storage(#[from] adlink_storage::StorageError),
    #[error("kernel error: {0}")]
    Kernel(#[from] adlink_kernel::KernelError),
    #[error("chunker error: {0}")]
    Chunker(#[from] ChunkerError),
    #[error("no matching ad accounts found for sync request")]
    NoTargetAccounts,
}

/// Request parameters controlling a multi-platform, long-date-range batch sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRequest {
    #[serde(default)]
    pub platforms: Vec<PlatformId>,
    #[serde(default)]
    pub ad_account_ids: Vec<String>,
    pub start_date: NaiveDate,
    pub end_date: NaiveDate,
    #[serde(default = "default_granularity")]
    pub granularity: ReportGranularity,
    #[serde(default = "default_levels")]
    pub levels: Vec<ReportLevel>,
    #[serde(default)]
    pub time_line: TimeLine,
    #[serde(default = "default_concurrency")]
    pub max_concurrency: usize,
    #[serde(default = "default_page_size")]
    pub page_size: u32,
    /// Optional existing job ID to resume skipped/incomplete checkpoints idempotently.
    #[serde(default)]
    pub resume_job_id: Option<String>,
    /// Override QPS per platform (defaults to platform's configured `default_qps`, or higher in mock mode).
    #[serde(default)]
    pub qps_override: Option<f64>,
}

fn default_granularity() -> ReportGranularity {
    ReportGranularity::Daily
}

fn default_levels() -> Vec<ReportLevel> {
    vec![ReportLevel::Account, ReportLevel::Campaign, ReportLevel::Creative]
}

fn default_concurrency() -> usize {
    8
}

fn default_page_size() -> u32 {
    200
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformSyncStat {
    pub platform: PlatformId,
    pub chunks_completed: usize,
    pub chunks_failed: usize,
    pub rows_upserted: usize,
}

/// Execution telemetry returned after completing a batch sync job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncJobSummary {
    pub job_id: String,
    pub status: String,
    pub total_chunks: usize,
    pub completed_chunks: usize,
    pub skipped_resumed_chunks: usize,
    pub failed_chunks: usize,
    pub rows_fetched: usize,
    pub rows_upserted: usize,
    pub elapsed_ms: u64,
    pub throughput_rows_per_sec: f64,
    pub by_platform: BTreeMap<String, PlatformSyncStat>,
    pub errors: Vec<String>,
}

#[derive(Clone)]
struct ChunkTask {
    account: AdAccount,
    connector: Arc<dyn AdPlatformConnector>,
    level: ReportLevel,
    chunk_start: NaiveDate,
    chunk_end: NaiveDate,
    qps: f64,
}

struct ChunkOutcome {
    platform: PlatformId,
    ad_account_id: String,
    level: ReportLevel,
    chunk_start: NaiveDate,
    chunk_end: NaiveDate,
    rows_fetched: usize,
    rows_upserted: usize,
    error: Option<String>,
}

/// Coordinates high-concurrency report and fund-statement synchronization across all mounted connectors.
pub struct SyncCoordinator {
    kernel: Arc<Kernel>,
    rate_limiter: Arc<RateLimiterPool>,
    retry_policy: RetryPolicy,
}

impl SyncCoordinator {
    pub fn new(kernel: Arc<Kernel>) -> Self {
        Self {
            kernel,
            rate_limiter: Arc::new(RateLimiterPool::new()),
            retry_policy: RetryPolicy::default(),
        }
    }

    pub fn with_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = policy;
        self
    }

    /// Executes a full or resumed multi-platform batch sync job concurrently.
    pub async fn sync_reports(&self, req: SyncRequest) -> Result<SyncJobSummary, SyncError> {
        let started_clock = Instant::now();
        let storage = self.kernel.require_service::<dyn StorageService>()?;

        let all_accounts = storage.list_ad_accounts(None).await?;
        let target_accounts: Vec<AdAccount> = all_accounts
            .into_iter()
            .filter(|a| req.platforms.is_empty() || req.platforms.contains(&a.platform))
            .filter(|a| req.ad_account_ids.is_empty() || req.ad_account_ids.contains(&a.id))
            .collect();

        if target_accounts.is_empty() {
            return Err(SyncError::NoTargetAccounts);
        }

        let job_id = req
            .resume_job_id
            .clone()
            .unwrap_or_else(|| format!("sync-{}", Uuid::new_v4()));

        let mut already_done = HashSet::new();
        if let Some(prev_id) = &req.resume_job_id {
            for cp in storage.list_sync_checkpoints(prev_id).await? {
                if cp.status == "completed" {
                    already_done.insert((
                        cp.ad_account_id,
                        cp.level,
                        cp.chunk_start,
                        cp.chunk_end,
                    ));
                }
            }
        }

        let levels = if req.levels.is_empty() {
            default_levels()
        } else {
            req.levels.clone()
        };

        let mut tasks = Vec::new();
        let mut skipped_resumed = 0usize;
        let mut missing_connectors = Vec::new();

        for acct in &target_accounts {
            let Some(connector) = self
                .kernel
                .get_named_service::<dyn AdPlatformConnector>(acct.platform.as_str())
            else {
                missing_connectors.push(acct.platform.as_str().to_string());
                continue;
            };
            let meta = connector.meta();
            let max_days = if req.granularity == ReportGranularity::Hourly {
                1
            } else {
                meta.max_sync_days_per_chunk
            };
            let chunks = DateRangeChunker::split(req.start_date, req.end_date, max_days)?;
            let effective_qps = req.qps_override.unwrap_or_else(|| {
                if connector.mode_label() == "mock" {
                    // Mock connectors run in-memory; allow rapid burst throughput while exercising limiter.
                    200.0
                } else {
                    meta.default_qps
                }
            });

            for &level in &levels {
                // Content-order platforms (PGY/Huahuo) naturally map to ContentOrder/Creative;
                // skip Account-only duplication only if connector chooses, otherwise keep uniform.
                for &(chunk_start, chunk_end) in &chunks {
                    if already_done.contains(&(acct.id.clone(), level, chunk_start, chunk_end)) {
                        skipped_resumed += 1;
                        continue;
                    }
                    tasks.push(ChunkTask {
                        account: acct.clone(),
                        connector: Arc::clone(&connector),
                        level,
                        chunk_start,
                        chunk_end,
                        qps: effective_qps,
                    });
                }
            }
        }

        let total_chunks = tasks.len() + skipped_resumed;
        let platforms_set: Vec<PlatformId> = target_accounts
            .iter()
            .map(|a| a.platform)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        let started_at = Utc::now();
        storage
            .upsert_sync_job(&SyncJobRecord {
                id: job_id.clone(),
                status: "running".into(),
                platforms: platforms_set.clone(),
                start_date: req.start_date,
                end_date: req.end_date,
                total_chunks,
                completed_chunks: skipped_resumed,
                failed_chunks: 0,
                rows_upserted: 0,
                started_at,
                finished_at: None,
                error_summary: None,
            })
            .await?;

        let concurrency = req.max_concurrency.clamp(1, 64);
        let page_size = req.page_size.clamp(10, 1000);
        let granularity = req.granularity;
        let time_line = req.time_line;
        let kernel = Arc::clone(&self.kernel);
        let limiter = Arc::clone(&self.rate_limiter);
        let retry = self.retry_policy.clone();
        let storage_ref = Arc::clone(&storage);
        let job_id_ref = job_id.clone();

        let outcomes: Vec<ChunkOutcome> = stream::iter(tasks)
            .map(|task| {
                let kernel = Arc::clone(&kernel);
                let limiter = Arc::clone(&limiter);
                let retry = retry.clone();
                let storage = Arc::clone(&storage_ref);
                let job_id = job_id_ref.clone();

                async move {
                    run_single_chunk(
                        &job_id,
                        task,
                        granularity,
                        time_line,
                        page_size,
                        &kernel,
                        &limiter,
                        &retry,
                        &storage,
                    )
                    .await
                }
            })
            .buffer_unordered(concurrency)
            .collect()
            .await;

        let mut completed_chunks = skipped_resumed;
        let mut failed_chunks = 0usize;
        let mut rows_fetched = 0usize;
        let mut rows_upserted = 0usize;
        let mut errors = missing_connectors;
        let mut by_platform: BTreeMap<String, PlatformSyncStat> = BTreeMap::new();

        for out in outcomes {
            let stat = by_platform
                .entry(out.platform.as_str().to_string())
                .or_insert_with(|| PlatformSyncStat {
                    platform: out.platform,
                    chunks_completed: 0,
                    chunks_failed: 0,
                    rows_upserted: 0,
                });

            let cp_status = if out.error.is_none() {
                completed_chunks += 1;
                stat.chunks_completed += 1;
                rows_fetched += out.rows_fetched;
                rows_upserted += out.rows_upserted;
                stat.rows_upserted += out.rows_upserted;
                "completed"
            } else {
                failed_chunks += 1;
                stat.chunks_failed += 1;
                if let Some(e) = &out.error {
                    errors.push(format!(
                        "{} {} [{}..{}]: {}",
                        out.platform, out.ad_account_id, out.chunk_start, out.chunk_end, e
                    ));
                }
                "failed"
            };

            let _ = storage
                .upsert_sync_checkpoint(&SyncCheckpoint {
                    job_id: job_id.clone(),
                    platform: out.platform,
                    ad_account_id: out.ad_account_id,
                    level: out.level,
                    chunk_start: out.chunk_start,
                    chunk_end: out.chunk_end,
                    status: cp_status.into(),
                    rows_fetched: out.rows_fetched,
                    error_message: out.error,
                    updated_at: Utc::now(),
                })
                .await;
        }

        let status = if failed_chunks == 0 && errors.is_empty() {
            "completed".to_string()
        } else if completed_chunks > 0 {
            "partial_failure".to_string()
        } else {
            "failed".to_string()
        };

        let elapsed_ms = started_clock.elapsed().as_millis().max(1) as u64;
        let throughput = (rows_upserted as f64) / (elapsed_ms as f64 / 1000.0);

        let error_summary = if errors.is_empty() {
            None
        } else {
            Some(errors.join("; "))
        };

        storage
            .upsert_sync_job(&SyncJobRecord {
                id: job_id.clone(),
                status: status.clone(),
                platforms: platforms_set,
                start_date: req.start_date,
                end_date: req.end_date,
                total_chunks,
                completed_chunks,
                failed_chunks,
                rows_upserted,
                started_at,
                finished_at: Some(Utc::now()),
                error_summary,
            })
            .await?;

        self.kernel.emit(KernelEvent::new(
            "sync.completed",
            "engine.sync_coordinator",
            serde_json::json!({
                "job_id": job_id,
                "status": status,
                "rows_upserted": rows_upserted,
                "elapsed_ms": elapsed_ms,
            }),
        ));

        Ok(SyncJobSummary {
            job_id,
            status,
            total_chunks,
            completed_chunks,
            skipped_resumed_chunks: skipped_resumed,
            failed_chunks,
            rows_fetched,
            rows_upserted,
            elapsed_ms,
            throughput_rows_per_sec: (throughput * 100.0).round() / 100.0,
            by_platform,
            errors,
        })
    }

    /// Synchronizes daily wallet / cash-vs-grant fund statements across accounts.
    pub async fn sync_fund_statements(
        &self,
        platforms: Vec<PlatformId>,
        ad_account_ids: Vec<String>,
        start_date: NaiveDate,
        end_date: NaiveDate,
    ) -> Result<usize, SyncError> {
        let storage = self.kernel.require_service::<dyn StorageService>()?;
        let accounts: Vec<AdAccount> = storage
            .list_ad_accounts(None)
            .await?
            .into_iter()
            .filter(|a| platforms.is_empty() || platforms.contains(&a.platform))
            .filter(|a| ad_account_ids.is_empty() || ad_account_ids.contains(&a.id))
            .collect();

        let mut total = 0usize;
        for acct in accounts {
            let Some(connector) = self
                .kernel
                .get_named_service::<dyn AdPlatformConnector>(acct.platform.as_str())
            else {
                continue;
            };
            let qps = if connector.mode_label() == "mock" {
                200.0
            } else {
                connector.meta().default_qps
            };
            self.rate_limiter
                .acquire_hierarchical(acct.platform.as_str(), &acct.id, qps)
                .await;
            let statements = self
                .retry_policy
                .execute(|| connector.fetch_fund_statements(&acct, start_date, end_date))
                .await;
            if let Ok(rows) = statements {
                total += storage.upsert_fund_statements(&rows).await?;
            }
        }
        Ok(total)
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_single_chunk(
    _job_id: &str,
    task: ChunkTask,
    granularity: ReportGranularity,
    time_line: TimeLine,
    page_size: u32,
    kernel: &Kernel,
    limiter: &RateLimiterPool,
    retry: &RetryPolicy,
    storage: &Arc<dyn StorageService>,
) -> ChunkOutcome {
    let mut page = 1u32;
    let mut chunk_rows: Vec<UnifiedReportRow> = Vec::new();

    loop {
        limiter
            .acquire_hierarchical(
                task.account.platform.as_str(),
                &task.account.id,
                task.qps,
            )
            .await;

        let req = ReportFetchRequest {
            account: task.account.clone(),
            start_date: task.chunk_start,
            end_date: task.chunk_end,
            granularity,
            level: task.level,
            time_line,
            page,
            page_size,
        };

        if let Err(err) = kernel
            .run_hooks(
                HookPoint::BeforeApiRequest,
                serde_json::json!({
                    "platform": task.account.platform.as_str(),
                    "ad_account_id": task.account.id,
                    "start_date": task.chunk_start.to_string(),
                    "end_date": task.chunk_end.to_string(),
                    "level": task.level.as_str(),
                    "page": page,
                }),
            )
            .await
        {
            return ChunkOutcome {
                platform: task.account.platform,
                ad_account_id: task.account.id,
                level: task.level,
                chunk_start: task.chunk_start,
                chunk_end: task.chunk_end,
                rows_fetched: chunk_rows.len(),
                rows_upserted: 0,
                error: Some(err.to_string()),
            };
        }

        let res = retry.execute(|| task.connector.fetch_reports_page(&req)).await;
        match res {
            Ok(resp) => {
                if kernel.has_hooks(HookPoint::AfterNormalizeRow) {
                    for row in resp.rows {
                        let Ok(val) = serde_json::to_value(&row) else {
                            chunk_rows.push(row);
                            continue;
                        };
                        match kernel.run_hooks(HookPoint::AfterNormalizeRow, val).await {
                            Ok(modified) => {
                                match serde_json::from_value::<UnifiedReportRow>(modified) {
                                    Ok(next_row) => chunk_rows.push(next_row),
                                    Err(_) => chunk_rows.push(row),
                                }
                            }
                            Err(adlink_kernel::KernelError::HookRejected { code, reason, .. }) => {
                                tracing::debug!(code = %code, reason = %reason, "row filtered by AfterNormalizeRow hook");
                            }
                            Err(_) => chunk_rows.push(row),
                        }
                    }
                } else {
                    chunk_rows.extend(resp.rows);
                }
                if !resp.has_more || page >= 500 {
                    break;
                }
                page += 1;
            }
            Err(err) => {
                return ChunkOutcome {
                    platform: task.account.platform,
                    ad_account_id: task.account.id,
                    level: task.level,
                    chunk_start: task.chunk_start,
                    chunk_end: task.chunk_end,
                    rows_fetched: chunk_rows.len(),
                    rows_upserted: 0,
                    error: Some(err.to_string()),
                };
            }
        }
    }

    let fetched = chunk_rows.len();
    match storage.upsert_reports(&chunk_rows).await {
        Ok(upserted) => ChunkOutcome {
            platform: task.account.platform,
            ad_account_id: task.account.id,
            level: task.level,
            chunk_start: task.chunk_start,
            chunk_end: task.chunk_end,
            rows_fetched: fetched,
            rows_upserted: upserted,
            error: None,
        },
        Err(err) => ChunkOutcome {
            platform: task.account.platform,
            ad_account_id: task.account.id,
            level: task.level,
            chunk_start: task.chunk_start,
            chunk_end: task.chunk_end,
            rows_fetched: fetched,
            rows_upserted: 0,
            error: Some(err.to_string()),
        },
    }
}
