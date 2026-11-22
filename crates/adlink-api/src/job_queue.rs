//! Bounded async sync worker queue, completion webhook dispatcher, and optional periodic scheduler.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use adlink_domain::{PlatformId, ReportGranularity, ReportLevel};
use adlink_engine::{SyncCoordinator, SyncRequest};
use adlink_kernel::Kernel;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, Mutex as AsyncMutex};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::metrics::MetricsRegistry;
use crate::server_config::ProductionServerConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AsyncSyncJobStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedSyncJobRecord {
    pub job_id: String,
    pub status: AsyncSyncJobStatus,
    pub enqueued_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub request_summary: Value,
    pub result: Option<Value>,
    pub error: Option<String>,
}

struct QueuedJobTask {
    job_id: String,
    req: SyncRequest,
}

/// Bounded worker queue for offloading multi-platform report syncs (`202 Accepted` -> poll `/api/v1/sync/jobs/{id}`).
pub struct SyncJobQueue {
    sender: Mutex<Option<mpsc::Sender<QueuedJobTask>>>,
    jobs: Arc<DashMap<String, QueuedSyncJobRecord>>,
    pending_count: Arc<AtomicUsize>,
    metrics: Arc<MetricsRegistry>,
    shutdown_tx: broadcast::Sender<()>,
    handles: Mutex<Vec<JoinHandle<()>>>,
}

impl SyncJobQueue {
    pub fn new(
        kernel: Arc<Kernel>,
        cfg: &ProductionServerConfig,
        metrics: Arc<MetricsRegistry>,
    ) -> Arc<Self> {
        let capacity = cfg.job_queue_capacity.max(1);
        let workers = cfg.job_worker_concurrency.max(1);
        let (tx, rx) = mpsc::channel::<QueuedJobTask>(capacity);
        let shared_rx = Arc::new(AsyncMutex::new(rx));
        let jobs = Arc::new(DashMap::<String, QueuedSyncJobRecord>::new());
        let pending_count = Arc::new(AtomicUsize::new(0));
        let (shutdown_tx, _) = broadcast::channel::<()>(4);
        let webhook_client = cfg.webhook_url.as_ref().map(|_| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new())
        });

        let mut handles = Vec::with_capacity(workers + 1);
        for worker_idx in 0..workers {
            let rx = Arc::clone(&shared_rx);
            let k = Arc::clone(&kernel);
            let jobs_map = Arc::clone(&jobs);
            let pending = Arc::clone(&pending_count);
            let m = Arc::clone(&metrics);
            let wh_url = cfg.webhook_url.clone();
            let wh_client = webhook_client.clone();

            handles.push(tokio::spawn(async move {
                loop {
                    let task = {
                        let mut locked = rx.lock().await;
                        locked.recv().await
                    };
                    let Some(task) = task else {
                        tracing::debug!(worker_idx, "sync job worker exiting on channel close");
                        break;
                    };
                    pending.fetch_sub(1, Ordering::Relaxed);
                    m.jobs_in_flight.fetch_add(1, Ordering::Relaxed);

                    if let Some(mut rec) = jobs_map.get_mut(&task.job_id) {
                        rec.status = AsyncSyncJobStatus::Running;
                        rec.started_at = Some(Utc::now());
                    }

                    let coord = SyncCoordinator::new(Arc::clone(&k));
                    let outcome = coord.sync_reports(task.req).await;
                    let finished_at = Utc::now();

                    match outcome {
                        Ok(summary) => {
                            m.jobs_completed_total.fetch_add(1, Ordering::Relaxed);
                            let res_val = serde_json::to_value(&summary).unwrap_or(Value::Null);
                            if let Some(mut rec) = jobs_map.get_mut(&task.job_id) {
                                rec.status = AsyncSyncJobStatus::Completed;
                                rec.finished_at = Some(finished_at);
                                rec.result = Some(res_val.clone());
                            }
                            if let (Some(url), Some(client)) = (&wh_url, &wh_client) {
                                dispatch_webhook(client, url, &task.job_id, "completed", Some(res_val), None).await;
                            }
                        }
                        Err(e) => {
                            m.jobs_failed_total.fetch_add(1, Ordering::Relaxed);
                            let msg = e.to_string();
                            if let Some(mut rec) = jobs_map.get_mut(&task.job_id) {
                                rec.status = AsyncSyncJobStatus::Failed;
                                rec.finished_at = Some(finished_at);
                                rec.error = Some(msg.clone());
                            }
                            if let (Some(url), Some(client)) = (&wh_url, &wh_client) {
                                dispatch_webhook(client, url, &task.job_id, "failed", None, Some(msg)).await;
                            }
                        }
                    }

                    m.jobs_in_flight.fetch_sub(1, Ordering::Relaxed);
                }
            }));
        }

        let queue = Arc::new(Self {
            sender: Mutex::new(Some(tx)),
            jobs,
            pending_count,
            metrics,
            shutdown_tx,
            handles: Mutex::new(handles),
        });

        if cfg.scheduled_sync_interval_secs > 0 {
            let interval = Duration::from_secs(cfg.scheduled_sync_interval_secs);
            let q = Arc::clone(&queue);
            let mut rx_stop = queue.shutdown_tx.subscribe();
            let sched_handle = tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(interval) => {
                            let end = Utc::now().date_naive();
                            let start = end - ChronoDuration::days(1);
                            let req = SyncRequest {
                                platforms: PlatformId::ALL.to_vec(),
                                ad_account_ids: vec![],
                                start_date: start,
                                end_date: end,
                                granularity: ReportGranularity::Daily,
                                levels: vec![ReportLevel::Account, ReportLevel::Campaign, ReportLevel::Creative],
                                time_line: Default::default(),
                                max_concurrency: 8,
                                page_size: 200,
                                resume_job_id: None,
                                qps_override: None,
                            };
                            if let Err(err) = q.enqueue(req) {
                                tracing::warn!(%err, "scheduled sync skipped because queue is saturated");
                            }
                        }
                        _ = rx_stop.recv() => {
                            tracing::info!("periodic sync scheduler stopped cleanly");
                            break;
                        }
                    }
                }
            });
            queue.handles.lock().unwrap().push(sched_handle);
        }

        queue
    }

    pub fn enqueue(&self, req: SyncRequest) -> Result<QueuedSyncJobRecord, String> {
        let job_id = format!("qjob-{}", Uuid::new_v4().simple());
        let summary = json!({
            "platforms": req.platforms.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
            "start_date": req.start_date.to_string(),
            "end_date": req.end_date.to_string(),
            "granularity": req.granularity,
            "levels": req.levels,
        });
        let record = QueuedSyncJobRecord {
            job_id: job_id.clone(),
            status: AsyncSyncJobStatus::Queued,
            enqueued_at: Utc::now(),
            started_at: None,
            finished_at: None,
            request_summary: summary,
            result: None,
            error: None,
        };
        self.jobs.insert(job_id.clone(), record.clone());

        let send_res = {
            let guard = self.sender.lock().map_err(|_| "queue sender poisoned".to_string())?;
            let Some(tx) = guard.as_ref() else {
                self.jobs.remove(&job_id);
                return Err("sync job queue is shutting down".into());
            };
            tx.try_send(QueuedJobTask { job_id: job_id.clone(), req })
        };

        match send_res {
            Ok(()) => {
                self.pending_count.fetch_add(1, Ordering::Relaxed);
                self.metrics
                    .jobs_enqueued_total
                    .fetch_add(1, Ordering::Relaxed);
                Ok(record)
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.jobs.remove(&job_id);
                Err("sync job queue is full; try again shortly".into())
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.jobs.remove(&job_id);
                Err("sync job queue is closed".into())
            }
        }
    }

    pub fn get_job(&self, job_id: &str) -> Option<QueuedSyncJobRecord> {
        self.jobs.get(job_id).map(|e| e.value().clone())
    }

    pub fn list_jobs(&self, limit: usize) -> Vec<QueuedSyncJobRecord> {
        let mut all: Vec<QueuedSyncJobRecord> =
            self.jobs.iter().map(|e| e.value().clone()).collect();
        all.sort_by(|a, b| b.enqueued_at.cmp(&a.enqueued_at));
        all.truncate(limit);
        all
    }

    pub fn queue_depth(&self) -> usize {
        self.pending_count.load(Ordering::Relaxed)
    }

    /// Closes the job submission channel, notifies scheduler exit, and awaits worker completion.
    pub async fn shutdown(&self) {
        if let Ok(mut guard) = self.sender.lock() {
            guard.take();
        }
        let _ = self.shutdown_tx.send(());
        let handles = {
            if let Ok(mut h) = self.handles.lock() {
                std::mem::take(&mut *h)
            } else {
                Vec::new()
            }
        };
        for handle in handles {
            let _ = handle.await;
        }
    }
}

async fn dispatch_webhook(
    client: &reqwest::Client,
    url: &str,
    job_id: &str,
    status: &str,
    result: Option<Value>,
    error: Option<String>,
) {
    let payload = json!({
        "event": "sync_job_finished",
        "job_id": job_id,
        "status": status,
        "timestamp": Utc::now().to_rfc3339(),
        "result": result,
        "error": error,
    });
    if let Err(e) = client.post(url).json(&payload).send().await {
        tracing::warn!(%job_id, %url, error = %e, "failed to dispatch async sync webhook");
    }
}
