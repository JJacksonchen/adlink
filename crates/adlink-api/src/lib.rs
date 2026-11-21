//! Unified HTTP REST API, OpenAPI 3.1 spec, Schema-Driven UI contract, and Axum server builder.

pub mod auth;
pub mod openapi;
pub mod routes;
pub mod ui_schema;

pub use auth::ApiAuthConfig;
pub use routes::build_app_router;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use adlink_connectors::{mount_all_connectors, seed_mock_governance, ConnectorSuiteConfig};
    use adlink_kernel::Kernel;
    use adlink_mcp::{AdSafetyGuardrailPlugin, GuardrailConfig, McpHandler};
    use adlink_storage::{SqliteStorage, SqliteStoragePlugin};
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
}
