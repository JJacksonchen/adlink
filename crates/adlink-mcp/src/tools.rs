//! 19 token-efficient MCP tools grouped into composable toolsets covering onboarding,
//! high-concurrency sync, caliber analytics, governance, creative attribution, and guarded ad ops.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use adlink_domain::{
    AccountType, AdAccount, AdCreationRequest, AdMutationAction, AdMutationRequest, BindingRole,
    CaliberCategory, CreativeAsset, CreativeFormat, CreativeHookType, CreativePlatformLink,
    CreativeProductionContribution, CreativeTaxonomy, CustomCaliberDef,
    FallbackBootstrapProjectRequest, Fen, OrgType, Organization, Person, PersonAccountBinding,
    PersonRole, PlatformId, ProductionRole, Project, ProjectAccountLink, ProjectType,
    QueryDimension, RelabelAccountGovernanceRequest, ReportGranularity, ReportLevel, ReportQuery,
    SettlementMode, TimeLine, UnifiedReportRow,
};
use adlink_engine::{AdPlatformConnector, SyncCoordinator, SyncRequest};
use adlink_kernel::Kernel;
use adlink_storage::StorageService;
use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth_flows::{
    discover_platform_accounts_enriched, fallback_bootstrap_project, relabel_account_governance,
};
use crate::guardrail::{
    execute_guarded_ad_creation, execute_guarded_ad_mutation, get_ad_creation_disclosure,
};
use crate::protocol::{mcp_tool_err, mcp_tool_ok};

#[derive(Clone)]
pub struct ToolMeta {
    pub name: &'static str,
    pub toolset: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

pub fn all_tools(enabled_toolsets: Option<&HashSet<String>>) -> Vec<Value> {
    tool_catalog()
        .into_iter()
        .filter(|t| {
            enabled_toolsets
                .map(|set| set.is_empty() || set.contains(t.toolset))
                .unwrap_or(true)
        })
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": t.input_schema
            })
        })
        .collect()
}

fn tool_catalog() -> Vec<ToolMeta> {
    vec![
        ToolMeta {
            name: "adlink_platform_catalog",
            toolset: "onboarding",
            description: "List all 8 supported ad platforms (10 connectors), official API base URLs, max chunk windows, rate limits, and official field mappings.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "platform": { "type": "string", "description": "Optional filter for a single platform id" }
                }
            }),
        },
        ToolMeta {
            name: "adlink_onboard_guide",
            toolset: "onboarding",
            description: "Return step-by-step instructions (Tier-1 Official MCP Auth Bridge + governance relabel vs Tier-2 Direct API fallback, plus mandatory default-off Ad Creation user prompt & risk disclosure).",
            input_schema: json!({
                "type": "object",
                "required": ["platform"],
                "properties": {
                    "platform": { "type": "string", "description": "Platform id e.g. tencent_ads, xhs_juguang, alipay_denghuo, ocean_engine_ad" }
                }
            }),
        },
        ToolMeta {
            name: "adlink_discover_platform_accounts",
            toolset: "onboarding",
            description: "Discover authorized advertiser/sub-accounts for a platform using Official MCP Bridge (optimal) or Direct Marketing API (fallback), enriched with current AdLink person/project governance bindings.",
            input_schema: json!({
                "type": "object",
                "required": ["platform"],
                "properties": {
                    "platform": { "type": "string", "description": "Platform id to inspect and list authorized accounts" }
                }
            }),
        },
        ToolMeta {
            name: "adlink_relabel_account_governance",
            toolset: "governance",
            description: "Optimal Tier-1 workflow: reuse an account authenticated via Official Platform MCP Bridge (or API) and freely re-label (重标) its internal 人-户关系 (PersonAccountBinding) and 项目关系 (ProjectAccountLink).",
            input_schema: json!({
                "type": "object",
                "required": ["platform", "platform_account_id"],
                "properties": {
                    "platform": { "type": "string" },
                    "platform_account_id": { "type": "string" },
                    "ad_account_id": { "type": "string" },
                    "account_name": { "type": "string" },
                    "auth_strategy": { "type": "string", "enum": ["official_mcp_bridge", "direct_api_account_discovery"] },
                    "official_mcp_endpoint": { "type": "string" },
                    "rebate_rate": { "type": "number" },
                    "service_fee_rate": { "type": "number" },
                    "default_cash_ratio": { "type": "number" },
                    "replace_existing_bindings": { "type": "boolean", "default": false },
                    "person_bindings": { "type": "array", "items": { "type": "object" } },
                    "replace_existing_project_links": { "type": "boolean", "default": false },
                    "project_links": { "type": "array", "items": { "type": "object" } }
                }
            }),
        },
        ToolMeta {
            name: "adlink_fallback_bootstrap_project",
            toolset: "governance",
            description: "Fallback Tier-2 workflow: create/update a custom project and bind pulled sub-accounts (+ optional owner person bindings) in one step when Official MCP Auth Bridge is not used.",
            input_schema: json!({
                "type": "object",
                "required": ["project", "accounts"],
                "properties": {
                    "project": { "type": "object" },
                    "accounts": { "type": "array", "items": { "type": "object" } },
                    "replace_existing_links": { "type": "boolean", "default": false }
                }
            }),
        },
        ToolMeta {
            name: "adlink_connect_account",
            toolset: "onboarding",
            description: "Create or update an AdAccount governance record linking a platform account ID, rebate rate, service fee rate, and credential reference.",
            input_schema: json!({
                "type": "object",
                "required": ["platform", "platform_account_id", "account_name"],
                "properties": {
                    "id": { "type": "string" },
                    "org_id": { "type": "string" },
                    "platform": { "type": "string" },
                    "platform_account_id": { "type": "string" },
                    "account_name": { "type": "string" },
                    "rebate_rate": { "type": "number" },
                    "service_fee_rate": { "type": "number" },
                    "default_cash_ratio": { "type": "number" },
                    "credential_ref": { "type": "string" }
                }
            }),
        },
        ToolMeta {
            name: "adlink_health_check",
            toolset: "system",
            description: "Check health across the microkernel plugins, SQLite WAL storage row counts, and all 10 platform connectors.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolMeta {
            name: "adlink_sync_reports",
            toolset: "sync",
            description: "Run high-concurrency long-date-range report sync with automatic platform chunking, rate limiting, retries, and SQLite WAL checkpoints.",
            input_schema: json!({
                "type": "object",
                "required": ["start_date", "end_date"],
                "properties": {
                    "platforms": { "type": "array", "items": { "type": "string" } },
                    "ad_account_ids": { "type": "array", "items": { "type": "string" } },
                    "start_date": { "type": "string", "description": "YYYY-MM-DD" },
                    "end_date": { "type": "string", "description": "YYYY-MM-DD" },
                    "granularity": { "type": "string", "enum": ["daily", "hourly"] },
                    "levels": { "type": "array", "items": { "type": "string" } },
                    "max_concurrency": { "type": "integer", "minimum": 1, "maximum": 64 },
                    "resume_job_id": { "type": "string" }
                }
            }),
        },
        ToolMeta {
            name: "adlink_sync_funds",
            toolset: "sync",
            description: "Sync daily cash vs grant wallet fund statements across ad accounts for cash/grant cost reconciliation.",
            input_schema: json!({
                "type": "object",
                "required": ["start_date", "end_date"],
                "properties": {
                    "platforms": { "type": "array", "items": { "type": "string" } },
                    "ad_account_ids": { "type": "array", "items": { "type": "string" } },
                    "start_date": { "type": "string" },
                    "end_date": { "type": "string" }
                }
            }),
        },
        ToolMeta {
            name: "adlink_ingest_external_records",
            toolset: "sync",
            description: "Ingest structured report or KOL order records (ideal for XHS Pugongying & Bilibili Huahuo exports with note_id/bvid) into unified_reports idempotently.",
            input_schema: json!({
                "type": "object",
                "required": ["records"],
                "properties": {
                    "records": { "type": "array", "items": { "type": "object" } }
                }
            }),
        },
        ToolMeta {
            name: "adlink_list_sync_jobs",
            toolset: "sync",
            description: "List recent batch sync jobs with chunk progress, throughput, and failure summaries.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "default": 20 }
                }
            }),
        },
        ToolMeta {
            name: "adlink_query_metrics",
            toolset: "analytics",
            description: "Query unified ad metrics grouped by date, platform, account, project, project_type, person (respecting split weights & effective dates), entity, or content_id.",
            input_schema: json!({
                "type": "object",
                "required": ["start_date", "end_date"],
                "properties": {
                    "start_date": { "type": "string" },
                    "end_date": { "type": "string" },
                    "platforms": { "type": "array", "items": { "type": "string" } },
                    "ad_account_ids": { "type": "array", "items": { "type": "string" } },
                    "project_ids": { "type": "array", "items": { "type": "string" } },
                    "person_ids": { "type": "array", "items": { "type": "string" } },
                    "levels": { "type": "array", "items": { "type": "string" } },
                    "group_by": { "type": "array", "items": { "type": "string", "enum": ["date", "platform", "account", "project", "project_type", "person", "entity", "content"] } }
                }
            }),
        },
        ToolMeta {
            name: "adlink_define_custom_caliber",
            toolset: "analytics",
            description: "Define and validate a custom metric caliber formula (自定义口径) using AdLink's safe AST DSL (e.g. safe_div(gmv_yuan, cash_cost_yuan + kol_order_fee_yuan)).",
            input_schema: json!({
                "type": "object",
                "required": ["name", "display_name", "formula"],
                "properties": {
                    "name": { "type": "string" },
                    "display_name": { "type": "string" },
                    "category": { "type": "string", "enum": ["spend", "performance", "efficiency", "custom"] },
                    "formula": { "type": "string" },
                    "unit": { "type": "string" },
                    "description": { "type": "string" }
                }
            }),
        },
        ToolMeta {
            name: "adlink_reconcile_calibers",
            toolset: "analytics",
            description: "Reconcile Spend Calibers (消耗口径: 账面/现金/净消耗/全域成本) and Performance Calibers (业绩口径: 毛流水/净营收/GMV/自定义口径) side-by-side.",
            input_schema: json!({
                "type": "object",
                "required": ["start_date", "end_date"],
                "properties": {
                    "start_date": { "type": "string" },
                    "end_date": { "type": "string" },
                    "platforms": { "type": "array", "items": { "type": "string" } },
                    "project_ids": { "type": "array", "items": { "type": "string" } }
                }
            }),
        },
        ToolMeta {
            name: "adlink_manage_org_people",
            toolset: "governance",
            description: "List or upsert organizations, internal people (optimizers/creators/AMs), and effective-dated person-account bindings (人-户关系).",
            input_schema: json!({
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": { "type": "string", "enum": ["list", "upsert_org", "upsert_person", "bind_person_account"] },
                    "data": { "type": "object" }
                }
            }),
        },
        ToolMeta {
            name: "adlink_manage_projects",
            toolset: "governance",
            description: "List or upsert marketing projects with project_type classification (项目类型分类) and project-account links.",
            input_schema: json!({
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": { "type": "string", "enum": ["list", "upsert_project", "link_account"] },
                    "data": { "type": "object" }
                }
            }),
        },
        ToolMeta {
            name: "adlink_manage_creatives",
            toolset: "governance",
            description: "List or upsert creative assets (创意类型分类), cross-platform creative/note_id/bvid links, and internal creator work-hour contributions.",
            input_schema: json!({
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": { "type": "string", "enum": ["list", "upsert_asset", "link_platform", "add_contribution"] },
                    "data": { "type": "object" }
                }
            }),
        },
        ToolMeta {
            name: "adlink_creative_attribution_report",
            toolset: "analytics",
            description: "Compute creative ROI attribution, creator production work-hour yield (gmv_per_work_hour_yuan), and cross-platform KOL seeding + paid boost attribution.",
            input_schema: json!({
                "type": "object",
                "required": ["start_date", "end_date"],
                "properties": {
                    "start_date": { "type": "string" },
                    "end_date": { "type": "string" },
                    "project_id": { "type": "string" },
                    "format": { "type": "string" },
                    "hook_type": { "type": "string" }
                }
            }),
        },
        ToolMeta {
            name: "adlink_list_campaigns",
            toolset: "ops",
            description: "List cached or live campaigns, ad groups, and creatives for an account.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "platform": { "type": "string" },
                    "ad_account_id": { "type": "string" }
                }
            }),
        },
        ToolMeta {
            name: "adlink_ad_mutate",
            toolset: "ops",
            description: "Safely pause, resume, update budget, or update bid on campaigns/adgroups through AdSafetyGuardrailPlugin. Defaults to dry_run=true.",
            input_schema: json!({
                "type": "object",
                "required": ["platform", "ad_account_id", "entity_level", "entity_ids", "action", "reason"],
                "properties": {
                    "platform": { "type": "string" },
                    "ad_account_id": { "type": "string" },
                    "entity_level": { "type": "string", "enum": ["campaign", "ad_group", "creative"] },
                    "entity_ids": { "type": "array", "items": { "type": "string" } },
                    "action": { "type": "object", "description": "{\"type\":\"pause\"} | {\"type\":\"resume\"} | {\"type\":\"update_budget\",\"daily_budget_fen\":100000} | {\"type\":\"update_bid\",\"bid_fen\":3000}" },
                    "dry_run": { "type": "boolean", "default": true },
                    "reason": { "type": "string" },
                    "operator": { "type": "string", "default": "mcp-agent" }
                }
            }),
        },
        ToolMeta {
            name: "adlink_creation_status_and_risks",
            toolset: "ops",
            description: "Check whether opt-in Ad Creation (创编:新建计划/单元/创意) is enabled, retrieve the mandatory default user-confirmation prompt, 5 financial/policy risk warnings, and per-platform API support matrix.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolMeta {
            name: "adlink_ad_create",
            toolset: "ops",
            description: "OPT-IN ONLY Ad Creation (创编): create campaign/ad_group/creative. Disabled by default (enabled=false) until the user explicitly consents & acknowledges risks; defaults to dry_run=true and initial_status=PAUSED.",
            input_schema: json!({
                "type": "object",
                "required": ["platform", "ad_account_id", "entity_kind", "name", "reason"],
                "properties": {
                    "platform": { "type": "string" },
                    "ad_account_id": { "type": "string" },
                    "entity_kind": { "type": "string", "enum": ["campaign", "ad_group", "creative"] },
                    "name": { "type": "string" },
                    "parent_entity_id": { "type": "string" },
                    "marketing_goal": { "type": "string" },
                    "daily_budget_fen": { "type": "integer" },
                    "bid_fen": { "type": "integer" },
                    "targeting": { "type": "object" },
                    "creative_assets": { "type": "object" },
                    "landing_url": { "type": "string" },
                    "initial_status": { "type": "string", "default": "PAUSED" },
                    "dry_run": { "type": "boolean", "default": true },
                    "preview_confirmed": { "type": "boolean", "default": false },
                    "reason": { "type": "string" },
                    "operator": { "type": "string", "default": "mcp-agent" },
                    "platform_extra": { "type": "object" }
                }
            }),
        },
        ToolMeta {
            name: "adlink_audit_logs",
            toolset: "ops",
            description: "Retrieve immutable audit log entries for ad mutations and guardrail rejections.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "default": 30 }
                }
            }),
        },
        ToolMeta {
            name: "adlink_list_plugins",
            toolset: "system",
            description: "Inspect all mounted Cordis-style microkernel plugins, provided services, hooks, and live health.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
    ]
}

/// Dispatches an MCP `tools/call` invocation cleanly and returns MCP `CallToolResult`.
pub async fn dispatch_tool(kernel: &Arc<Kernel>, name: &str, args: Value) -> Value {
    match dispatch_inner(kernel, name, args).await {
        Ok(val) => mcp_tool_ok(val),
        Err(err) => mcp_tool_err(err),
    }
}

async fn dispatch_inner(kernel: &Arc<Kernel>, name: &str, args: Value) -> Result<Value, String> {
    let storage = kernel
        .require_service::<dyn StorageService>()
        .map_err(|e| e.to_string())?;

    match name {
        "adlink_platform_catalog" => {
            if let Some(p_str) = args.get("platform").and_then(Value::as_str) {
                let p: PlatformId = p_str.parse().map_err(|e: adlink_domain::ParsePlatformError| e.to_string())?;
                return Ok(serde_json::to_value(p.meta()).map_err(|e| e.to_string())?);
            }
            let all: Vec<_> = PlatformId::ALL.iter().map(|p| p.meta()).collect();
            Ok(json!({ "count": all.len(), "platforms": all }))
        }
        "adlink_onboard_guide" => {
            let p_str = args
                .get("platform")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing required field 'platform'".to_string())?;
            let p: PlatformId = p_str.parse().map_err(|e: adlink_domain::ParsePlatformError| e.to_string())?;
            let conn = kernel
                .get_named_service::<dyn AdPlatformConnector>(p.as_str())
                .ok_or_else(|| format!("connector '{}' is not mounted", p.as_str()))?;
            let mut val = serde_json::to_value(conn.onboard_guide()).map_err(|e| e.to_string())?;
            if let Some(obj) = val.as_object_mut() {
                obj.insert(
                    "ad_creation_policy".into(),
                    serde_json::to_value(get_ad_creation_disclosure(kernel))
                        .map_err(|e| e.to_string())?,
                );
            }
            Ok(val)
        }
        "adlink_discover_platform_accounts" => {
            let p_str = args
                .get("platform")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing required field 'platform'".to_string())?;
            let p: PlatformId = p_str.parse().map_err(|e: adlink_domain::ParsePlatformError| e.to_string())?;
            discover_platform_accounts_enriched(kernel, p).await
        }
        "adlink_relabel_account_governance" => {
            let req: RelabelAccountGovernanceRequest =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            relabel_account_governance(kernel, "mcp", req).await
        }
        "adlink_fallback_bootstrap_project" => {
            let req: FallbackBootstrapProjectRequest =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            fallback_bootstrap_project(kernel, "mcp", req).await
        }
        "adlink_connect_account" => {
            #[derive(Deserialize)]
            struct Req {
                id: Option<String>,
                org_id: Option<String>,
                platform: PlatformId,
                platform_account_id: String,
                account_name: String,
                rebate_rate: Option<f64>,
                service_fee_rate: Option<f64>,
                default_cash_ratio: Option<f64>,
                credential_ref: Option<String>,
            }
            let r: Req = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let acct = AdAccount {
                id: r.id.unwrap_or_else(|| format!("acc-{}-{}", r.platform.as_str(), r.platform_account_id)),
                org_id: r.org_id.unwrap_or_else(|| "org-starpulse".into()),
                platform: r.platform,
                platform_account_id: r.platform_account_id,
                account_name: r.account_name,
                account_type: AccountType::AgencySub,
                rebate_rate: r.rebate_rate.unwrap_or(0.05),
                service_fee_rate: r.service_fee_rate.unwrap_or(0.08),
                default_cash_ratio: r.default_cash_ratio.unwrap_or(0.9),
                status: "active".into(),
                credential_ref: r.credential_ref,
                extra: json!({}),
            };
            storage.upsert_ad_account(&acct).await.map_err(|e| e.to_string())?;
            Ok(json!({ "status": "connected", "account": acct }))
        }
        "adlink_health_check" => {
            let kernel_health = kernel.health_report().await;
            let db = storage.db_health_summary().await.map_err(|e| e.to_string())?;
            let connectors: Vec<_> = PlatformId::ALL
                .iter()
                .filter_map(|p| {
                    kernel
                        .get_named_service::<dyn AdPlatformConnector>(p.as_str())
                        .map(|c| json!({ "platform": p.as_str(), "mode": c.mode_label() }))
                })
                .collect();
            Ok(json!({
                "healthy": kernel_health.healthy,
                "plugins_mounted": kernel_health.plugin_count,
                "storage": db,
                "connectors": connectors
            }))
        }
        "adlink_sync_reports" => {
            let req: SyncRequest = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let coord = SyncCoordinator::new(Arc::clone(kernel));
            let summary = coord.sync_reports(req).await.map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(summary).map_err(|e| e.to_string())?)
        }
        "adlink_sync_funds" => {
            #[derive(Deserialize)]
            struct Req {
                #[serde(default)]
                platforms: Vec<PlatformId>,
                #[serde(default)]
                ad_account_ids: Vec<String>,
                start_date: NaiveDate,
                end_date: NaiveDate,
            }
            let r: Req = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let coord = SyncCoordinator::new(Arc::clone(kernel));
            let count = coord
                .sync_fund_statements(r.platforms, r.ad_account_ids, r.start_date, r.end_date)
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({ "fund_statements_upserted": count }))
        }
        "adlink_ingest_external_records" => {
            #[derive(Deserialize)]
            struct Req {
                records: Vec<UnifiedReportRow>,
            }
            let r: Req = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let count = storage
                .upsert_reports(&r.records)
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({ "ingested_rows": count }))
        }
        "adlink_list_sync_jobs" => {
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
            let jobs = storage.list_sync_jobs(limit).await.map_err(|e| e.to_string())?;
            Ok(json!({ "jobs": jobs }))
        }
        "adlink_query_metrics" => {
            let query: ReportQuery = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let (summary, groups) = storage
                .query_reconciled_metrics(&query)
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({ "summary": summary, "groups": groups }))
        }
        "adlink_define_custom_caliber" => {
            #[derive(Deserialize)]
            struct Req {
                name: String,
                display_name: String,
                category: Option<CaliberCategory>,
                formula: String,
                unit: Option<String>,
                description: Option<String>,
            }
            let r: Req = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let vars = adlink_domain::CaliberEvaluator::validate_formula(&r.formula)
                .map_err(|e| format!("invalid caliber formula: {e}"))?;
            let def = CustomCaliberDef {
                id: format!("cal-{}", r.name),
                name: r.name,
                display_name: r.display_name,
                category: r.category.unwrap_or(CaliberCategory::Custom),
                formula: r.formula,
                unit: r.unit.unwrap_or_else(|| "ratio".into()),
                description: r.description.unwrap_or_default(),
                enabled: true,
            };
            storage
                .upsert_custom_caliber(&def)
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({ "caliber": def, "referenced_variables": vars }))
        }
        "adlink_reconcile_calibers" => {
            #[derive(Deserialize)]
            struct Req {
                start_date: NaiveDate,
                end_date: NaiveDate,
                #[serde(default)]
                platforms: Vec<PlatformId>,
                #[serde(default)]
                project_ids: Vec<String>,
            }
            let r: Req = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let (summary, by_platform) = storage
                .query_reconciled_metrics(&ReportQuery {
                    start_date: r.start_date,
                    end_date: r.end_date,
                    platforms: r.platforms,
                    project_ids: r.project_ids,
                    group_by: vec![QueryDimension::Platform],
                    ..Default::default()
                })
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({ "reconciliation_total": summary, "by_platform": by_platform }))
        }
        "adlink_manage_org_people" => {
            let action = args
                .get("action")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing action".to_string())?;
            let data = args.get("data").cloned().unwrap_or(Value::Null);
            match action {
                "list" => Ok(json!({
                    "organizations": storage.list_organizations().await.map_err(|e| e.to_string())?,
                    "people": storage.list_people().await.map_err(|e| e.to_string())?,
                    "person_account_bindings": storage.list_person_account_bindings().await.map_err(|e| e.to_string())?,
                })),
                "upsert_org" => {
                    let org: Organization = serde_json::from_value(data).unwrap_or(Organization {
                        id: format!("org-{}", Uuid::new_v4()),
                        name: "New Org".into(),
                        org_type: OrgType::Agency,
                        default_currency: "CNY".into(),
                    });
                    storage.upsert_organization(&org).await.map_err(|e| e.to_string())?;
                    Ok(json!({ "organization": org }))
                }
                "upsert_person" => {
                    let p: Person = serde_json::from_value(data).map_err(|e| e.to_string())?;
                    storage.upsert_person(&p).await.map_err(|e| e.to_string())?;
                    Ok(json!({ "person": p }))
                }
                "bind_person_account" => {
                    #[derive(Deserialize)]
                    struct BindIn {
                        id: Option<String>,
                        person_id: String,
                        ad_account_id: String,
                        relation_role: Option<BindingRole>,
                        split_weight: Option<f64>,
                        effective_from: NaiveDate,
                        effective_to: Option<NaiveDate>,
                    }
                    let b: BindIn = serde_json::from_value(data).map_err(|e| e.to_string())?;
                    let binding = PersonAccountBinding {
                        id: b.id.unwrap_or_else(|| format!("bind-{}", Uuid::new_v4())),
                        person_id: b.person_id,
                        ad_account_id: b.ad_account_id,
                        relation_role: b.relation_role.unwrap_or(BindingRole::PrimaryOptimizer),
                        split_weight: b.split_weight.unwrap_or(1.0),
                        effective_from: b.effective_from,
                        effective_to: b.effective_to,
                    };
                    storage
                        .upsert_person_account_binding(&binding)
                        .await
                        .map_err(|e| e.to_string())?;
                    Ok(json!({ "binding": binding }))
                }
                _ => Err(format!("unknown action '{action}'")),
            }
        }
        "adlink_manage_projects" => {
            let action = args
                .get("action")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing action".to_string())?;
            let data = args.get("data").cloned().unwrap_or(Value::Null);
            match action {
                "list" => Ok(json!({
                    "projects": storage.list_projects().await.map_err(|e| e.to_string())?,
                    "project_account_links": storage.list_project_account_links().await.map_err(|e| e.to_string())?,
                })),
                "upsert_project" => {
                    let p: Project = serde_json::from_value(data).map_err(|e| e.to_string())?;
                    storage.upsert_project(&p).await.map_err(|e| e.to_string())?;
                    Ok(json!({ "project": p }))
                }
                "link_account" => {
                    let l: ProjectAccountLink =
                        serde_json::from_value(data).map_err(|e| e.to_string())?;
                    storage
                        .upsert_project_account_link(&l)
                        .await
                        .map_err(|e| e.to_string())?;
                    Ok(json!({ "link": l }))
                }
                _ => Err(format!("unknown action '{action}'")),
            }
        }
        "adlink_manage_creatives" => {
            let action = args
                .get("action")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing action".to_string())?;
            let data = args.get("data").cloned().unwrap_or(Value::Null);
            match action {
                "list" => Ok(json!({
                    "assets": storage.list_creative_assets().await.map_err(|e| e.to_string())?,
                    "platform_links": storage.list_creative_platform_links().await.map_err(|e| e.to_string())?,
                    "contributions": storage.list_creative_contributions().await.map_err(|e| e.to_string())?,
                })),
                "upsert_asset" => {
                    let a: CreativeAsset = serde_json::from_value(data).map_err(|e| e.to_string())?;
                    storage.upsert_creative_asset(&a).await.map_err(|e| e.to_string())?;
                    Ok(json!({ "asset": a }))
                }
                "link_platform" => {
                    let l: CreativePlatformLink =
                        serde_json::from_value(data).map_err(|e| e.to_string())?;
                    storage
                        .upsert_creative_platform_link(&l)
                        .await
                        .map_err(|e| e.to_string())?;
                    Ok(json!({ "link": l }))
                }
                "add_contribution" => {
                    let c: CreativeProductionContribution =
                        serde_json::from_value(data).map_err(|e| e.to_string())?;
                    storage
                        .upsert_creative_contribution(&c)
                        .await
                        .map_err(|e| e.to_string())?;
                    Ok(json!({ "contribution": c }))
                }
                _ => Err(format!("unknown action '{action}'")),
            }
        }
        "adlink_creative_attribution_report" => {
            #[derive(Deserialize)]
            struct Req {
                start_date: NaiveDate,
                end_date: NaiveDate,
                project_id: Option<String>,
                format: Option<CreativeFormat>,
                hook_type: Option<CreativeHookType>,
            }
            let r: Req = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let (creatives, creators, seeding_boost) = storage
                .query_creative_attribution(
                    r.start_date,
                    r.end_date,
                    r.project_id.as_deref(),
                    r.format,
                    r.hook_type,
                )
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({
                "creative_attribution": creatives,
                "creator_workload_summary": creators,
                "seeding_boost_attribution": seeding_boost
            }))
        }
        "adlink_list_campaigns" => {
            let platform = args
                .get("platform")
                .and_then(Value::as_str)
                .map(|s| s.parse::<PlatformId>())
                .transpose()
                .map_err(|e| e.to_string())?;
            let acct_id = args.get("ad_account_id").and_then(Value::as_str);
            let entities = storage
                .list_ad_entities(platform, acct_id)
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({ "count": entities.len(), "entities": entities }))
        }
        "adlink_ad_mutate" => {
            let req: AdMutationRequest = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let res = execute_guarded_ad_mutation(kernel, "mcp", req).await?;
            Ok(serde_json::to_value(res).map_err(|e| e.to_string())?)
        }
        "adlink_creation_status_and_risks" => {
            serde_json::to_value(get_ad_creation_disclosure(kernel)).map_err(|e| e.to_string())
        }
        "adlink_ad_create" => {
            let req: AdCreationRequest = serde_json::from_value(args).map_err(|e| e.to_string())?;
            match execute_guarded_ad_creation(kernel, "mcp", req).await {
                Ok(res) => serde_json::to_value(res).map_err(|e| e.to_string()),
                Err(err) => Err(serde_json::to_string_pretty(&json!({
                    "code": err.code,
                    "message": err.message,
                    "is_opt_in_block": err.is_opt_in_block,
                    "default_ask_user_prompt": err.disclosure.default_ask_user_prompt,
                    "risk_warnings": err.disclosure.risk_warnings,
                    "how_to_enable_toml": err.disclosure.how_to_enable_toml
                }))
                .unwrap_or_else(|_| err.to_string())),
            }
        }
        "adlink_audit_logs" => {
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(30) as usize;
            let logs = storage.list_audit_logs(limit).await.map_err(|e| e.to_string())?;
            Ok(json!({ "audit_logs": logs }))
        }
        "adlink_list_plugins" => {
            let report = kernel.health_report().await;
            let hooks: Vec<_> = kernel
                .list_hooks()
                .into_iter()
                .map(|(pt, pid, hid, prio)| {
                    json!({ "hook_point": pt, "plugin_id": pid, "hook_id": hid, "priority": prio })
                })
                .collect();
            let services: Vec<_> = kernel
                .describe_services()
                .into_iter()
                .map(|(pid, ty, name)| {
                    json!({ "plugin_id": pid, "type": ty, "name": name })
                })
                .collect();
            Ok(json!({
                "kernel_healthy": report.healthy,
                "plugins": report.plugins,
                "hooks": hooks,
                "services": services
            }))
        }
        other => Err(format!("unknown MCP tool '{other}'")),
    }
}

// Keep unused imports clean by anchoring domain types referenced via serde deserialization.
#[allow(dead_code)]
fn _touch_types(
    _: PersonRole,
    _: ProductionRole,
    _: ProjectType,
    _: SettlementMode,
    _: ReportGranularity,
    _: ReportLevel,
    _: TimeLine,
    _: BTreeMap<String, Value>,
    _: AdMutationAction,
    _: Fen,
    _: CreativeTaxonomy,
) {
}
