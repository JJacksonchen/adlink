//! Headless REST API handlers and unified Axum router composition.

use std::path::PathBuf;
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
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::{json, Value};
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::auth::{api_key_middleware, ApiAuthConfig};
use crate::openapi::openapi_spec;
use crate::ui_schema::ui_schema;

#[derive(Clone)]
pub struct ApiState {
    pub kernel: Arc<Kernel>,
}

fn api_err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

/// Builds the complete unified HTTP router combining:
/// - MCP Streamable HTTP (`/mcp`) & Legacy SSE (`/sse`, `/messages`)
/// - Headless REST API (`/api/v1/*`) & OpenAPI 3.1 (`/api/v1/openapi.json`)
/// - Customizable static Web console (`web_dir`, if present)
pub fn build_app_router(
    kernel: Arc<Kernel>,
    mcp_handler: Arc<McpHandler>,
    auth: ApiAuthConfig,
    web_dir: Option<PathBuf>,
) -> Router {
    let state = ApiState { kernel };

    let api_routes = Router::new()
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

    router
        .layer(middleware::from_fn_with_state(auth, api_key_middleware))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
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
        "plugins_mounted": report.plugin_count,
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
    match storage.query_reconciled_metrics(&query).await {
        Ok((summary, groups)) => Json(json!({ "summary": summary, "groups": groups })).into_response(),
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
    match storage.query_reconciled_metrics(&query).await {
        Ok((summary, by_group)) => {
            Json(json!({ "reconciliation_total": summary, "groups": by_group })).into_response()
        }
        Err(e) => api_err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

#[derive(Deserialize)]
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
        Ok((creatives, creators, seeding_boost)) => Json(json!({
            "creative_attribution": creatives,
            "creator_workload_summary": creators,
            "seeding_boost_attribution": seeding_boost
        }))
        .into_response(),
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
    match storage.list_sync_jobs(30).await {
        Ok(jobs) => Json(json!({ "jobs": jobs })).into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct IngestReq {
    records: Vec<UnifiedReportRow>,
}

async fn ingest_external(State(state): State<ApiState>, Json(req): Json<IngestReq>) -> Response {
    let Ok(storage) = state.kernel.require_service::<dyn StorageService>() else {
        return api_err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable");
    };
    match storage.upsert_reports(&req.records).await {
        Ok(count) => Json(json!({ "ingested_rows": count })).into_response(),
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
    Json(req): Json<AdMutationRequest>,
) -> Response {
    match execute_guarded_ad_mutation(&state.kernel, "rest_api", req).await {
        Ok(res) => Json(json!(res)).into_response(),
        Err(msg) => api_err(StatusCode::BAD_REQUEST, msg),
    }
}

async fn ad_creation_status_handler(State(state): State<ApiState>) -> Response {
    Json(json!(get_ad_creation_disclosure(&state.kernel))).into_response()
}

async fn ad_creation_create_handler(
    State(state): State<ApiState>,
    Json(req): Json<AdCreationRequest>,
) -> Response {
    match execute_guarded_ad_creation(&state.kernel, "rest_api", req).await {
        Ok(res) => Json(json!(res)).into_response(),
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
