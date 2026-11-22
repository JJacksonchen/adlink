//! Headless REST API handlers and unified commercial Axum router composition.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use adlink_domain::{
    AdAccount, AdCreationRequest, AdMutationRequest, CreativeAsset, CreativeFormat,
    CreativeHookType, CustomCaliberDef, FallbackBootstrapProjectRequest, Person,
    PersonAccountBinding, PlatformId, Project, ProjectAccountLink, RelabelAccountGovernanceRequest,
    ReportQuery, UnifiedReportRow,
};
use adlink_engine::{AdPlatformConnector, SyncCoordinator, SyncRequest};
use adlink_kernel::Kernel;
use adlink_mcp::{
    discover_platform_accounts_enriched, execute_guarded_ad_creation, execute_guarded_ad_mutation,
    fallback_bootstrap_project, get_ad_creation_disclosure, mcp_http_router,
    relabel_account_governance, McpHandler,
};
use adlink_storage::StorageService;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::{json, Value};
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::auth::{api_key_middleware, ApiAuthConfig};
use crate::cache::{AnalyticsQueryCache, IdempotencyStore};
use crate::job_queue::SyncJobQueue;
use crate::metrics::MetricsRegistry;
use crate::middleware::{
    load_shed_middleware, observability_and_timeout_middleware, rate_limit_middleware,
    MiddlewareState,
};
use crate::openapi::openapi_spec;
use crate::server_config::ProductionServerConfig;
use crate::ui_schema::ui_schema;

#[derive(Clone)]
pub struct ApiState {
    pub kernel: Arc<Kernel>,
    pub metrics: Arc<MetricsRegistry>,
    pub analytics_cache: Arc<AnalyticsQueryCache>,
    pub idempotency: Arc<IdempotencyStore>,
    pub job_queue: Arc<SyncJobQueue>,
    pub draining: Arc<AtomicBool>,
}

/// Handle returned by [`build_production_app`] for health draining, telemetry, and graceful shutdown.
pub struct ProductionRuntime {
    pub metrics: Arc<MetricsRegistry>,
    pub job_queue: Arc<SyncJobQueue>,
    pub draining: Arc<AtomicBool>,
}

impl ProductionRuntime {
    /// Marks the instance as draining so Kubernetes `/readyz` stops routing new traffic (`503`).
    pub fn begin_drain(&self) {
        self.draining.store(true, Ordering::Release);
    }

    /// Drains background async sync workers cleanly before process exit.
    pub async fn shutdown(&self) {
        self.begin_drain();
        self.job_queue.shutdown().await;
    }
}

fn api_err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn idempotency_key_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn json_with_header(status: StatusCode, body: Value, header_name: &'static str, header_val: &'static str) -> Response {
    let mut resp = (status, Json(body)).into_response();
    resp.headers_mut()
        .insert(header_name, HeaderValue::from_static(header_val));
    resp
}

/// Backward-compatible entrypoint returning a ready [`Router`] with default commercial settings.
pub fn build_app_router(
    kernel: Arc<Kernel>,
    mcp_handler: Arc<McpHandler>,
    auth: ApiAuthConfig,
    web_dir: Option<PathBuf>,
) -> Router {
    build_production_app(
        kernel,
        mcp_handler,
        auth,
        web_dir,
        ProductionServerConfig::default(),
    )
    .0
}

/// Builds the commercial high-concurrency HTTP + MCP router together with its runtime control handle:
/// - Kubernetes/Prometheus probes (`/healthz`, `/readyz`, `/metrics`)
/// - MCP Streamable HTTP (`/mcp`) & Legacy SSE (`/sse`, `/messages`)
/// - Headless REST API (`/api/v1/*`, async queue `/api/v1/sync/jobs/enqueue`)
/// - Epoch-invalidated analytics cache, `Idempotency-Key` deduplication, rate limiting (`429`),
///   adaptive load shedding (`503`), request timeouts (`504`), body limits, and security headers
pub fn build_production_app(
    kernel: Arc<Kernel>,
    mcp_handler: Arc<McpHandler>,
    auth: ApiAuthConfig,
    web_dir: Option<PathBuf>,
    server_cfg: ProductionServerConfig,
) -> (Router, Arc<ProductionRuntime>) {
    let cfg = Arc::new(server_cfg);
    let metrics = Arc::new(MetricsRegistry::new());
    let analytics_cache = Arc::new(AnalyticsQueryCache::new(
        cfg.analytics_cache_ttl_secs,
        cfg.analytics_cache_max_entries,
    ));
    let idempotency = Arc::new(IdempotencyStore::new(cfg.idempotency_ttl_secs));
    let job_queue = SyncJobQueue::new(Arc::clone(&kernel), &cfg, Arc::clone(&metrics));
    let draining = Arc::new(AtomicBool::new(false));

    let state = ApiState {
        kernel,
        metrics: Arc::clone(&metrics),
        analytics_cache,
        idempotency,
        job_queue: Arc::clone(&job_queue),
        draining: Arc::clone(&draining),
    };
    let mw_state = MiddlewareState::new(Arc::clone(&cfg), Arc::clone(&metrics));

    let api_routes = Router::new()
        .route("/healthz", get(liveness_handler))
        .route("/readyz", get(readiness_handler))
        .route("/metrics", get(metrics_handler))
        .route("/api/v1/health", get(health_handler))
        .route("/api/v1/openapi.json", get(openapi_handler))
        .route("/api/v1/ui/schema", get(ui_schema_handler))
        .route("/api/v1/plugins", get(plugins_handler))
        .route("/api/v1/platforms", get(platforms_handler))
        .route("/api/v1/platforms/{id}/onboard", get(platform_onboard_handler))
        .route("/api/v1/platforms/{id}/discover-accounts", get(discover_platform_accounts_handler))
        .route("/api/v1/governance/relabel", post(relabel_governance_handler))
        .route("/api/v1/governance/fallback-bootstrap", post(fallback_bootstrap_handler))
        .route("/api/v1/accounts", get(list_accounts).post(upsert_account))
        .route("/api/v1/people", get(list_people).post(upsert_person))
        .route("/api/v1/bindings", get(list_bindings).post(upsert_binding))
        .route("/api/v1/projects", get(list_projects).post(upsert_project))
        .route("/api/v1/projects/links", post(upsert_project_link))
        .route("/api/v1/creatives", get(list_creatives).post(upsert_creative))
        .route("/api/v1/calibers", get(list_calibers).post(upsert_caliber))
        .route("/api/v1/analytics/query", post(analytics_query))
        .route("/api/v1/analytics/reconcile", post(analytics_reconcile))
        .route("/api/v1/attribution/creatives", post(creative_attribution))
        .route("/api/v1/sync/reports", post(trigger_sync_reports))
        .route("/api/v1/sync/funds", post(trigger_sync_funds))
        .route("/api/v1/sync/jobs", get(list_sync_jobs))
        .route("/api/v1/sync/jobs/enqueue", post(enqueue_sync_job))
        .route("/api/v1/sync/jobs/{id}", get(get_enqueued_sync_job))
        .route("/api/v1/ingest", post(ingest_external))
        .route("/api/v1/ad-ops/campaigns", get(list_ad_entities))
        .route("/api/v1/ad-ops/mutate", post(mutate_ad_entities))
        .route("/api/v1/ad-creation/status", get(ad_creation_status_handler))
        .route("/api/v1/ad-creation/create", post(ad_creation_create_handler))
        .route("/api/v1/audit-logs", get(list_audit_logs))
        .with_state(state);

    let mut router = api_routes.merge(mcp_http_router(mcp_handler));

    if let Some(dir) = web_dir {
        if dir.exists() {
            router = router.fallback_service(ServeDir::new(dir).append_index_html_on_directories(true));
        }
    }

    let cors = if cfg.cors_allowed_origins.is_empty() {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    } else {
        let origins: Vec<HeaderValue> = cfg
            .cors_allowed_origins
            .iter()
            .filter_map(|o| HeaderValue::from_str(o.trim()).ok())
            .collect();
        if origins.is_empty() {
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any)
        } else {
            CorsLayer::new()
                .allow_origin(AllowOrigin::list(origins))
                .allow_methods(Any)
                .allow_headers(Any)
        }
    };

    let app = router
        .layer(DefaultBodyLimit::max(cfg.max_body_bytes.max(1024)))
        .layer(middleware::from_fn_with_state(auth, api_key_middleware))
        .layer(middleware::from_fn_with_state(
            mw_state.clone(),
            rate_limit_middleware,
        ))
        .layer(middleware::from_fn_with_state(
            mw_state.clone(),
            load_shed_middleware,
        ))
        .layer(middleware::from_fn_with_state(
            mw_state,
            observability_and_timeout_middleware,
        ))
        .layer(cors)
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http());

    let runtime = Arc::new(ProductionRuntime {
        metrics,
        job_queue,
        draining,
    });
    (app, runtime)
}

async fn liveness_handler() -> Response {
    Json(json!({
        "status": "alive",
        "service": "adlink",
        "version": env!("CARGO_PKG_VERSION")
    }))
    .into_response()
}

async fn readiness_handler(State(state): State<ApiState>) -> Response {
    if state.draining.load(Ordering::Acquire) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "status": "draining",
                "ready": false
            })),
        )
            .into_response();
    }
    let Ok(storage) = state.kernel.require_service::<dyn StorageService>() else {
        return api_err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable");
    };
    match storage.count_report_rows().await {
        Ok(rows) => Json(json!({
            "status": "ready",
            "ready": true,
            "report_rows": rows,
            "pool": storage.pool_stats(),
            "queue_depth": state.job_queue.queue_depth(),
        }))
        .into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "status": "not_ready",
                "ready": false,
                "error": e.to_string()
            })),
        )
            .into_response(),
    }
}

async fn metrics_handler(State(state): State<ApiState>) -> Response {
    let (epoch, pool_readers) = match state.kernel.require_service::<dyn StorageService>() {
        Ok(s) => {
            let stats = s.pool_stats();
            let readers = stats
                .get("reader_pool_size")
                .and_then(Value::as_u64)
                .unwrap_or(1) as usize;
            (s.write_epoch(), readers)
        }
        Err(_) => (0, 0),
    };
    let text = state.metrics.render_prometheus(
        epoch,
        pool_readers,
        state.job_queue.queue_depth(),
        state.draining.load(Ordering::Acquire),
    );
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        text,
    )
        .into_response()
}

async fn health_handler(State(state): State<ApiState>) -> Response {
    let report = state.kernel.health_report().await;
    let db = match state.kernel.require_service::<dyn StorageService>() {
        Ok(s) => s.db_health_summary().await.unwrap_or(Value::Null),
        Err(_) => Value::Null,
    };
    Json(json!({
        "status": if report.healthy { "ok" } else { "degraded" },
        "version": env!("CARGO_PKG_VERSION"),
        "draining": state.draining.load(Ordering::Acquire),
        "plugins_mounted": report.plugin_count,
        "queue_depth": state.job_queue.queue_depth(),
        "storage": db
    }))
    .into_response()
}

async fn openapi_handler() -> Json<Value> {
    Json(openapi_spec())
}

async fn ui_schema_handler() -> Json<Value> {
    Json(ui_schema())
}

async fn plugins_handler(State(state): State<ApiState>) -> Json<Value> {
    let report = state.kernel.health_report().await;
    let services: Vec<_> = state
        .kernel
        .describe_services()
        .into_iter()
        .map(|(pid, ty, name)| json!({ "plugin_id": pid, "type": ty, "name": name }))
        .collect();
    let hooks: Vec<_> = state
        .kernel
        .list_hooks()
        .into_iter()
        .map(|(pt, pid, hid, prio)| {
            json!({ "hook_point": pt, "plugin_id": pid, "hook_id": hid, "priority": prio })
        })
        .collect();
    Json(json!({
        "healthy": report.healthy,
        "plugins": report.plugins,
        "services": services,
        "hooks": hooks
    }))
}

async fn platforms_handler(State(state): State<ApiState>) -> Json<Value> {
    let list: Vec<Value> = PlatformId::ALL
        .iter()
        .map(|p| {
            let mode = state
                .kernel
                .get_named_service::<dyn AdPlatformConnector>(p.as_str())
                .map(|c| c.mode_label())
                .unwrap_or("unmounted");
            let meta = p.meta();
            json!({
                "meta": meta,
                "mode": mode
            })
        })
        .collect();
    Json(json!({ "count": list.len(), "platforms": list }))
}

async fn platform_onboard_handler(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Response {
    let Ok(platform) = id.parse::<PlatformId>() else {
        return api_err(StatusCode::NOT_FOUND, format!("unknown platform '{id}'"));
    };
    let Some(conn) = state
        .kernel
        .get_named_service::<dyn AdPlatformConnector>(platform.as_str())
    else {
        return api_err(StatusCode::SERVICE_UNAVAILABLE, "connector not mounted");
    };
    Json(json!(conn.onboard_guide())).into_response()
}

async fn discover_platform_accounts_handler(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Response {
    let Ok(platform) = id.parse::<PlatformId>() else {
        return api_err(StatusCode::NOT_FOUND, format!("unknown platform '{id}'"));
    };
    match discover_platform_accounts_enriched(&state.kernel, platform).await {
        Ok(val) => Json(val).into_response(),
        Err(msg) => api_err(StatusCode::BAD_REQUEST, msg),
    }
}

async fn relabel_governance_handler(
    State(state): State<ApiState>,
    Json(req): Json<RelabelAccountGovernanceRequest>,
) -> Response {
    match relabel_account_governance(&state.kernel, "rest_api", req).await {
        Ok(val) => Json(val).into_response(),
        Err(msg) => api_err(StatusCode::BAD_REQUEST, msg),
    }
}

async fn fallback_bootstrap_handler(
    State(state): State<ApiState>,
    Json(req): Json<FallbackBootstrapProjectRequest>,
) -> Response {
    match fallback_bootstrap_project(&state.kernel, "rest_api", req).await {
        Ok(val) => Json(val).into_response(),
        Err(msg) => api_err(StatusCode::BAD_REQUEST, msg),
    }
}

async fn list_accounts(State(state): State<ApiState>) -> Response {
    let Ok(storage) = state.kernel.require_service::<dyn StorageService>() else {
        return api_err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable");
    };
    match storage.list_ad_accounts(None).await {
        Ok(accounts) => Json(json!({ "accounts": accounts })).into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn upsert_account(
    State(state): State<ApiState>,
    Json(acct): Json<AdAccount>,
) -> Response {
    let Ok(storage) = state.kernel.require_service::<dyn StorageService>() else {
        return api_err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable");
    };
    match storage.upsert_ad_account(&acct).await {
        Ok(()) => Json(json!({ "account": acct })).into_response(),
        Err(e) => api_err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn list_people(State(state): State<ApiState>) -> Response {
    let Ok(storage) = state.kernel.require_service::<dyn StorageService>() else {
        return api_err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable");
    };
    match storage.list_people().await {
        Ok(people) => Json(json!({ "people": people })).into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn upsert_person(State(state): State<ApiState>, Json(person): Json<Person>) -> Response {
    let Ok(storage) = state.kernel.require_service::<dyn StorageService>() else {
        return api_err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable");
    };
    match storage.upsert_person(&person).await {
        Ok(()) => Json(json!({ "person": person })).into_response(),
        Err(e) => api_err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn list_bindings(State(state): State<ApiState>) -> Response {
    let Ok(storage) = state.kernel.require_service::<dyn StorageService>() else {
        return api_err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable");
    };
    match storage.list_person_account_bindings().await {
        Ok(bindings) => Json(json!({ "bindings": bindings })).into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn upsert_binding(
    State(state): State<ApiState>,
    Json(binding): Json<PersonAccountBinding>,
) -> Response {
    let Ok(storage) = state.kernel.require_service::<dyn StorageService>() else {
        return api_err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable");
    };
    match storage.upsert_person_account_binding(&binding).await {
        Ok(()) => Json(json!({ "binding": binding })).into_response(),
        Err(e) => api_err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn list_projects(State(state): State<ApiState>) -> Response {
    let Ok(storage) = state.kernel.require_service::<dyn StorageService>() else {
        return api_err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable");
    };
    let projects = storage.list_projects().await.unwrap_or_default();
    let links = storage.list_project_account_links().await.unwrap_or_default();
    Json(json!({ "projects": projects, "project_account_links": links })).into_response()
}

async fn upsert_project(State(state): State<ApiState>, Json(project): Json<Project>) -> Response {
    let Ok(storage) = state.kernel.require_service::<dyn StorageService>() else {
        return api_err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable");
    };
    match storage.upsert_project(&project).await {
        Ok(()) => Json(json!({ "project": project })).into_response(),
        Err(e) => api_err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn upsert_project_link(
    State(state): State<ApiState>,
    Json(link): Json<ProjectAccountLink>,
) -> Response {
    let Ok(storage) = state.kernel.require_service::<dyn StorageService>() else {
        return api_err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable");
    };
    match storage.upsert_project_account_link(&link).await {
        Ok(()) => Json(json!({ "link": link })).into_response(),
        Err(e) => api_err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn list_creatives(State(state): State<ApiState>) -> Response {
    let Ok(storage) = state.kernel.require_service::<dyn StorageService>() else {
        return api_err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable");
    };
    let assets = storage.list_creative_assets().await.unwrap_or_default();
    let links = storage.list_creative_platform_links().await.unwrap_or_default();
    let contributions = storage.list_creative_contributions().await.unwrap_or_default();
    Json(json!({
        "assets": assets,
        "platform_links": links,
        "contributions": contributions
    }))
    .into_response()
}

async fn upsert_creative(
    State(state): State<ApiState>,
    Json(asset): Json<CreativeAsset>,
) -> Response {
    let Ok(storage) = state.kernel.require_service::<dyn StorageService>() else {
        return api_err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable");
    };
    match storage.upsert_creative_asset(&asset).await {
        Ok(()) => Json(json!({ "asset": asset })).into_response(),
        Err(e) => api_err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn list_calibers(State(state): State<ApiState>) -> Response {
    let Ok(storage) = state.kernel.require_service::<dyn StorageService>() else {
        return api_err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable");
    };
    match storage.list_custom_calibers().await {
        Ok(custom) => Json(json!({
            "spend_calibers": ["gross_media_cost", "cash_cost", "net_after_rebate_cost", "full_funnel_cost"],
            "performance_calibers": ["agency_gross_billing", "agency_net_revenue", "platform_gmv", "brand_net_roi"],
            "custom_calibers": custom
        }))
        .into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn upsert_caliber(
    State(state): State<ApiState>,
    Json(def): Json<CustomCaliberDef>,
) -> Response {
    let Ok(storage) = state.kernel.require_service::<dyn StorageService>() else {
        return api_err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable");
    };
    if let Err(err) = adlink_domain::CaliberEvaluator::validate_formula(&def.formula) {
        return api_err(StatusCode::BAD_REQUEST, format!("invalid formula: {err}"));
    }
    match storage.upsert_custom_caliber(&def).await {
        Ok(()) => Json(json!({ "caliber": def })).into_response(),
        Err(e) => api_err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn analytics_query(
    State(state): State<ApiState>,
    Json(query): Json<ReportQuery>,
) -> Response {
    let Ok(storage) = state.kernel.require_service::<dyn StorageService>() else {
        return api_err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable");
    };
    let epoch = storage.write_epoch();
    let cache_key = format!("query:{}", serde_json::to_string(&query).unwrap_or_default());
    if let Some(hit) = state.analytics_cache.get(&cache_key, epoch) {
        state.metrics.cache_hits_total.fetch_add(1, Ordering::Relaxed);
        return json_with_header(StatusCode::OK, hit, "x-cache", "HIT");
    }
    state.metrics.cache_misses_total.fetch_add(1, Ordering::Relaxed);

    match storage.query_reconciled_metrics(&query).await {
        Ok((summary, groups)) => {
            let body = json!({ "summary": summary, "groups": groups });
            state.analytics_cache.insert(cache_key, epoch, body.clone());
            json_with_header(StatusCode::OK, body, "x-cache", "MISS")
        }
        Err(e) => api_err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn analytics_reconcile(
    State(state): State<ApiState>,
    Json(mut query): Json<ReportQuery>,
) -> Response {
    let Ok(storage) = state.kernel.require_service::<dyn StorageService>() else {
        return api_err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable");
    };
    if query.group_by.is_empty() {
        query.group_by = vec![adlink_domain::QueryDimension::Platform];
    }
    let epoch = storage.write_epoch();
    let cache_key = format!(
        "reconcile:{}",
        serde_json::to_string(&query).unwrap_or_default()
    );
    if let Some(hit) = state.analytics_cache.get(&cache_key, epoch) {
        state.metrics.cache_hits_total.fetch_add(1, Ordering::Relaxed);
        return json_with_header(StatusCode::OK, hit, "x-cache", "HIT");
    }
    state.metrics.cache_misses_total.fetch_add(1, Ordering::Relaxed);

    match storage.query_reconciled_metrics(&query).await {
        Ok((summary, by_group)) => {
            let body = json!({ "reconciliation_total": summary, "groups": by_group });
            state.analytics_cache.insert(cache_key, epoch, body.clone());
            json_with_header(StatusCode::OK, body, "x-cache", "MISS")
        }
        Err(e) => api_err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

#[derive(serde::Serialize, Deserialize)]
struct CreativeAttrReq {
    start_date: NaiveDate,
    end_date: NaiveDate,
    project_id: Option<String>,
    format: Option<CreativeFormat>,
    hook_type: Option<CreativeHookType>,
}

async fn creative_attribution(
    State(state): State<ApiState>,
    Json(req): Json<CreativeAttrReq>,
) -> Response {
    let Ok(storage) = state.kernel.require_service::<dyn StorageService>() else {
        return api_err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable");
    };
    let epoch = storage.write_epoch();
    let cache_key = format!("creative_attr:{}", serde_json::to_string(&req).unwrap_or_default());
    if let Some(hit) = state.analytics_cache.get(&cache_key, epoch) {
        state.metrics.cache_hits_total.fetch_add(1, Ordering::Relaxed);
        return json_with_header(StatusCode::OK, hit, "x-cache", "HIT");
    }
    state.metrics.cache_misses_total.fetch_add(1, Ordering::Relaxed);

    match storage
        .query_creative_attribution(
            req.start_date,
            req.end_date,
            req.project_id.as_deref(),
            req.format,
            req.hook_type,
        )
        .await
    {
        Ok((creatives, creators, seeding_boost)) => {
            let body = json!({
                "creative_attribution": creatives,
                "creator_workload_summary": creators,
                "seeding_boost_attribution": seeding_boost
            });
            state.analytics_cache.insert(cache_key, epoch, body.clone());
            json_with_header(StatusCode::OK, body, "x-cache", "MISS")
        }
        Err(e) => api_err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn trigger_sync_reports(
    State(state): State<ApiState>,
    Json(req): Json<SyncRequest>,
) -> Response {
    let coord = SyncCoordinator::new(Arc::clone(&state.kernel));
    match coord.sync_reports(req).await {
        Ok(summary) => Json(json!(summary)).into_response(),
        Err(e) => api_err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

#[derive(Deserialize)]
struct FundSyncReq {
    #[serde(default)]
    platforms: Vec<PlatformId>,
    #[serde(default)]
    ad_account_ids: Vec<String>,
    start_date: NaiveDate,
    end_date: NaiveDate,
}

async fn trigger_sync_funds(
    State(state): State<ApiState>,
    Json(req): Json<FundSyncReq>,
) -> Response {
    let coord = SyncCoordinator::new(Arc::clone(&state.kernel));
    match coord
        .sync_fund_statements(req.platforms, req.ad_account_ids, req.start_date, req.end_date)
        .await
    {
        Ok(count) => Json(json!({ "fund_statements_upserted": count })).into_response(),
        Err(e) => api_err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn list_sync_jobs(State(state): State<ApiState>) -> Response {
    let Ok(storage) = state.kernel.require_service::<dyn StorageService>() else {
        return api_err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable");
    };
    let queued_jobs = state.job_queue.list_jobs(30);
    match storage.list_sync_jobs(30).await {
        Ok(jobs) => Json(json!({ "jobs": jobs, "queued_jobs": queued_jobs })).into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn enqueue_sync_job(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<SyncRequest>,
) -> Response {
    let idem_key = idempotency_key_header(&headers).map(|k| format!("sync_enqueue:{k}"));
    if let Some(ref scoped) = idem_key {
        if let Some((status_u16, body)) = state.idempotency.get(scoped) {
            state
                .metrics
                .idempotency_replays_total
                .fetch_add(1, Ordering::Relaxed);
            return json_with_header(
                StatusCode::from_u16(status_u16).unwrap_or(StatusCode::ACCEPTED),
                body,
                "x-idempotency-replayed",
                "true",
            );
        }
    }

    match state.job_queue.enqueue(req) {
        Ok(record) => {
            let body = json!(record);
            if let Some(scoped) = idem_key {
                state
                    .idempotency
                    .put(scoped, StatusCode::ACCEPTED.as_u16(), body.clone());
            }
            (StatusCode::ACCEPTED, Json(body)).into_response()
        }
        Err(msg) => {
            let mut resp = api_err(StatusCode::SERVICE_UNAVAILABLE, msg);
            resp.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
            resp
        }
    }
}

async fn get_enqueued_sync_job(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Response {
    match state.job_queue.get_job(&id) {
        Some(job) => Json(json!(job)).into_response(),
        None => api_err(
            StatusCode::NOT_FOUND,
            format!("queued sync job '{id}' not found"),
        ),
    }
}

#[derive(Deserialize)]
struct IngestReq {
    records: Vec<UnifiedReportRow>,
}

async fn ingest_external(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<IngestReq>,
) -> Response {
    let idem_key = idempotency_key_header(&headers).map(|k| format!("ingest:{k}"));
    if let Some(ref scoped) = idem_key {
        if let Some((status_u16, body)) = state.idempotency.get(scoped) {
            state
                .metrics
                .idempotency_replays_total
                .fetch_add(1, Ordering::Relaxed);
            return json_with_header(
                StatusCode::from_u16(status_u16).unwrap_or(StatusCode::OK),
                body,
                "x-idempotency-replayed",
                "true",
            );
        }
    }

    let Ok(storage) = state.kernel.require_service::<dyn StorageService>() else {
        return api_err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable");
    };
    match storage.upsert_reports(&req.records).await {
        Ok(count) => {
            let body = json!({ "ingested_rows": count });
            if let Some(scoped) = idem_key {
                state
                    .idempotency
                    .put(scoped, StatusCode::OK.as_u16(), body.clone());
            }
            Json(body).into_response()
        }
        Err(e) => api_err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

#[derive(Deserialize)]
struct CampaignQuery {
    platform: Option<PlatformId>,
    ad_account_id: Option<String>,
}

async fn list_ad_entities(
    State(state): State<ApiState>,
    Query(q): Query<CampaignQuery>,
) -> Response {
    let Ok(storage) = state.kernel.require_service::<dyn StorageService>() else {
        return api_err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable");
    };
    match storage
        .list_ad_entities(q.platform, q.ad_account_id.as_deref())
        .await
    {
        Ok(entities) => Json(json!({ "entities": entities })).into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn mutate_ad_entities(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<AdMutationRequest>,
) -> Response {
    let idem_key = idempotency_key_header(&headers).map(|k| format!("ad_mutate:{k}"));
    if let Some(ref scoped) = idem_key {
        if let Some((status_u16, body)) = state.idempotency.get(scoped) {
            state
                .metrics
                .idempotency_replays_total
                .fetch_add(1, Ordering::Relaxed);
            return json_with_header(
                StatusCode::from_u16(status_u16).unwrap_or(StatusCode::OK),
                body,
                "x-idempotency-replayed",
                "true",
            );
        }
    }
    match execute_guarded_ad_mutation(&state.kernel, "rest_api", req).await {
        Ok(res) => {
            let body = json!(res);
            if let Some(scoped) = idem_key {
                state
                    .idempotency
                    .put(scoped, StatusCode::OK.as_u16(), body.clone());
            }
            Json(body).into_response()
        }
        Err(msg) => api_err(StatusCode::BAD_REQUEST, msg),
    }
}

async fn ad_creation_status_handler(State(state): State<ApiState>) -> Response {
    Json(json!(get_ad_creation_disclosure(&state.kernel))).into_response()
}

async fn ad_creation_create_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<AdCreationRequest>,
) -> Response {
    let idem_key = idempotency_key_header(&headers).map(|k| format!("ad_create:{k}"));
    if let Some(ref scoped) = idem_key {
        if let Some((status_u16, body)) = state.idempotency.get(scoped) {
            state
                .metrics
                .idempotency_replays_total
                .fetch_add(1, Ordering::Relaxed);
            return json_with_header(
                StatusCode::from_u16(status_u16).unwrap_or(StatusCode::OK),
                body,
                "x-idempotency-replayed",
                "true",
            );
        }
    }
    match execute_guarded_ad_creation(&state.kernel, "rest_api", req).await {
        Ok(res) => {
            let body = json!(res);
            if let Some(scoped) = idem_key {
                state
                    .idempotency
                    .put(scoped, StatusCode::OK.as_u16(), body.clone());
            }
            Json(body).into_response()
        }
        Err(err) => {
            let status = if err.is_opt_in_block {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::BAD_REQUEST
            };
            (
                status,
                Json(json!({
                    "error": err.message,
                    "code": err.code,
                    "is_opt_in_block": err.is_opt_in_block,
                    "disclosure": err.disclosure
                })),
            )
                .into_response()
        }
    }
}

async fn list_audit_logs(State(state): State<ApiState>) -> Response {
    let Ok(storage) = state.kernel.require_service::<dyn StorageService>() else {
        return api_err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable");
    };
    match storage.list_audit_logs(50).await {
        Ok(logs) => Json(json!({ "audit_logs": logs })).into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}
