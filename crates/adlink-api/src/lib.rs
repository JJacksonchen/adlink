//! Unified HTTP REST API, OpenAPI 3.1 spec, Schema-Driven UI contract, and commercial Axum server builder.

pub mod auth;
pub mod cache;
pub mod job_queue;
pub mod metrics;
pub mod middleware;
pub mod openapi;
pub mod routes;
pub mod server_config;
pub mod ui_schema;

pub use auth::ApiAuthConfig;
pub use job_queue::{AsyncSyncJobStatus, QueuedSyncJobRecord, SyncJobQueue};
pub use metrics::MetricsRegistry;
pub use routes::{build_app_router, build_production_app, ProductionRuntime};
pub use server_config::ProductionServerConfig;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use adlink_connectors::{mount_all_connectors, seed_mock_governance, ConnectorSuiteConfig};
    use adlink_kernel::Kernel;
    use adlink_mcp::{AdSafetyGuardrailPlugin, GuardrailConfig, McpHandler};
    use adlink_storage::{SqliteStorage, SqliteStoragePlugin, StorageService};
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::{json, Value};
    use tempfile::tempdir;
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn api_routes_and_auth_middleware_work_with_axum_08_paths() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(SqliteStorage::open(dir.path().join("api.db")).unwrap());
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

        let mcp = Arc::new(McpHandler::new(Arc::clone(&kernel), None));
        let auth = ApiAuthConfig::from_keys(vec!["secret-token-1".into()]);
        let app = build_app_router(kernel, mcp, auth, None);

        // Health & OpenAPI stay accessible; protected route requires Bearer key.
        let health = app
            .clone()
            .oneshot(Request::builder().uri("/api/v1/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(health.status(), 200);

        let unauth = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/platforms/tencent_ads/onboard")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauth.status(), 401);

        let authed = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/platforms/tencent_ads/onboard")
                    .header("Authorization", "Bearer secret-token-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(authed.status(), 200);

        // Trigger sync and verify reconcile endpoint.
        let sync_res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/sync/reports")
                    .header("Authorization", "Bearer secret-token-1")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "platforms": ["tencent_ads", "alipay_denghuo"],
                            "start_date": "2026-11-01",
                            "end_date": "2026-11-05",
                            "levels": ["account", "creative"]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(sync_res.status(), 200);

        // Ad Creation is disabled by default -> status shows enabled=false & create returns 403 FORBIDDEN with disclosure.
        let ac_status = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/ad-creation/status")
                    .header("Authorization", "Bearer secret-token-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ac_status.status(), 200);

        let ac_create_blocked = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/ad-creation/create")
                    .header("Authorization", "Bearer secret-token-1")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "platform": "tencent_ads",
                            "ad_account_id": "acc-tencent-01",
                            "entity_kind": "ad_group",
                            "name": "API Blocked",
                            "daily_budget_fen": 100000,
                            "reason": "test default block"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ac_create_blocked.status(), 403);

        // Two-tier account discovery REST route works cleanly.
        let disc = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/platforms/tencent_ads/discover-accounts")
                    .header("Authorization", "Bearer secret-token-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(disc.status(), 200);
    }

    async fn setup_test_kernel() -> (Arc<Kernel>, Arc<SqliteStorage>, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let storage = Arc::new(SqliteStorage::open_with_pool(dir.path().join("prod.db"), 4).unwrap());
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
        (kernel, storage, dir)
    }

    #[tokio::test]
    async fn production_probes_metrics_cache_idempotency_and_async_queue_work() {
        let (kernel, storage, _dir) = setup_test_kernel().await;
        let mcp = Arc::new(McpHandler::new(Arc::clone(&kernel), None));
        let (app, runtime) = build_production_app(
            Arc::clone(&kernel),
            mcp,
            ApiAuthConfig::from_keys(vec!["prod-key".into()]),
            None,
            ProductionServerConfig::default(),
        );

        // 1. /healthz & /readyz & /metrics exempt from bearer auth, return 200.
        let hz = app
            .clone()
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(hz.status(), 200);

        let rz = app
            .clone()
            .oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(rz.status(), 200);

        // 2. Enqueue async sync job -> 202 Accepted -> poll until completed.
        let enq_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/sync/jobs/enqueue")
                    .header("Authorization", "Bearer prod-key")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "platforms": ["tiktok_ads", "meta_ads"],
                            "start_date": "2026-11-01",
                            "end_date": "2026-11-02",
                            "levels": ["account", "campaign"]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(enq_resp.status(), 202);
        let enq_bytes = axum::body::to_bytes(enq_resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let enq_json: Value = serde_json::from_slice(&enq_bytes).unwrap();
        let job_id = enq_json["job_id"].as_str().unwrap().to_string();

        let mut completed = false;
        for _ in 0..40 {
            let poll = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/v1/sync/jobs/{job_id}"))
                        .header("Authorization", "Bearer prod-key")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(poll.status(), 200);
            let b = axum::body::to_bytes(poll.into_body(), 1024 * 1024)
                .await
                .unwrap();
            let v: Value = serde_json::from_slice(&b).unwrap();
            if v["status"] == "completed" {
                completed = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(completed, "queued sync job did not complete in time");

        // 3. Analytics cache MISS -> HIT -> write epoch bump -> MISS.
        let query_body = json!({
            "start_date": "2026-11-01",
            "end_date": "2026-11-02",
            "group_by": ["platform"]
        })
        .to_string();

        let r1 = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/analytics/reconcile")
                    .header("Authorization", "Bearer prod-key")
                    .header("content-type", "application/json")
                    .body(Body::from(query_body.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r1.status(), 200);
        assert_eq!(r1.headers().get("x-cache").unwrap(), "MISS");
        assert!(r1.headers().contains_key("x-request-id"));

        let r2 = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/analytics/reconcile")
                    .header("Authorization", "Bearer prod-key")
                    .header("content-type", "application/json")
                    .body(Body::from(query_body.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r2.status(), 200);
        assert_eq!(r2.headers().get("x-cache").unwrap(), "HIT");

        // Trigger a storage write via Idempotent ingest -> bumps write_epoch and invalidates cache.
        let ingest_body = json!({
            "records": [{
                "platform": "tiktok_ads",
                "ad_account_id": "acc-tiktok-01",
                "platform_account_id": "7391028374619201",
                "stat_date": "2026-11-02",
                "stat_hour": -1,
                "granularity": "daily",
                "level": "campaign",
                "entity_id": "tt_camp_extra",
                "entity_name": "Extra TT",
                "gross_cost_fen": 72500,
                "cash_cost_fen": 72500,
                "grant_cost_fen": 0,
                "impressions": 5000,
                "clicks": 150,
                "conversions": 8,
                "orders": 8,
                "gmv_fen": 290000
            }]
        })
        .to_string();

        let ing1 = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/ingest")
                    .header("Authorization", "Bearer prod-key")
                    .header("Idempotency-Key", "idem-ingest-001")
                    .header("content-type", "application/json")
                    .body(Body::from(ingest_body.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ing1.status(), 200);
        assert!(ing1.headers().get("x-idempotency-replayed").is_none());

        let rows_after_first = storage.count_report_rows().await.unwrap();

        let ing2 = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/ingest")
                    .header("Authorization", "Bearer prod-key")
                    .header("Idempotency-Key", "idem-ingest-001")
                    .header("content-type", "application/json")
                    .body(Body::from(ingest_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ing2.status(), 200);
        assert_eq!(ing2.headers().get("x-idempotency-replayed").unwrap(), "true");
        assert_eq!(storage.count_report_rows().await.unwrap(), rows_after_first);

        let r3 = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/analytics/reconcile")
                    .header("Authorization", "Bearer prod-key")
                    .header("content-type", "application/json")
                    .body(Body::from(query_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r3.status(), 200);
        assert_eq!(r3.headers().get("x-cache").unwrap(), "MISS");

        // 4. Metrics endpoint exports Prometheus gauges & counters.
        let metrics_resp = app
            .clone()
            .oneshot(Request::builder().uri("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(metrics_resp.status(), 200);
        let metrics_text = String::from_utf8(
            axum::body::to_bytes(metrics_resp.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(metrics_text.contains("adlink_http_requests_total"));
        assert!(metrics_text.contains("adlink_storage_write_epoch"));
        assert!(metrics_text.contains("adlink_cache_hits_total 1"));

        // 5. Draining causes /readyz -> 503.
        runtime.shutdown().await;
        let rz_draining = app
            .oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(rz_draining.status(), 503);
    }

    #[tokio::test]
    async fn rate_limiter_429_and_load_shedder_503_protect_server() {
        let (kernel, _storage, _dir) = setup_test_kernel().await;
        let mcp = Arc::new(McpHandler::new(Arc::clone(&kernel), None));

        // Test Rate Limiter with burst=2
        let (rl_app, rl_runtime) = build_production_app(
            Arc::clone(&kernel),
            Arc::clone(&mcp),
            ApiAuthConfig::default(),
            None,
            ProductionServerConfig {
                rate_limit_rps: 1,
                rate_limit_burst: 2,
                ..Default::default()
            },
        );
        let req = || {
            Request::builder()
                .uri("/api/v1/platforms")
                .header("x-forwarded-for", "10.0.0.42")
                .body(Body::empty())
                .unwrap()
        };
        assert_eq!(rl_app.clone().oneshot(req()).await.unwrap().status(), 200);
        assert_eq!(rl_app.clone().oneshot(req()).await.unwrap().status(), 200);
        assert_eq!(rl_app.oneshot(req()).await.unwrap().status(), 429);
        rl_runtime.shutdown().await;

        // Test Adaptive Load Shedding with max_in_flight_requests=0
        let (shed_app, shed_runtime) = build_production_app(
            kernel,
            mcp,
            ApiAuthConfig::default(),
            None,
            ProductionServerConfig {
                max_in_flight_requests: 0,
                ..Default::default()
            },
        );
        let shed_resp = shed_app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/platforms")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(shed_resp.status(), 503);
        // Probes remain reachable even under load shedding!
        let hz_resp = shed_app
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(hz_resp.status(), 200);
        shed_runtime.shutdown().await;
    }
}
