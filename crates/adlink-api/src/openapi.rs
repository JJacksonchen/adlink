//! OpenAPI 3.1 specification generator for self-built agents, Dify/Coze plugins, Web, and MiniProgram SDKs.

use serde_json::{json, Value};

pub fn openapi_spec() -> Value {
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "AdLink Enterprise Ad Data & MCP Middleware API",
            "version": env!("CARGO_PKG_VERSION"),
            "license": {
                "name": "Apache-2.0",
                "url": "https://www.apache.org/licenses/LICENSE-2.0"
            },
            "description": "Headless REST & MCP API unifying 8 mainstream Chinese ad platforms (Tencent Ads v3.0, XHS Juguang/Chengfeng/Pugongying, Bilibili Sanlian/Huahuo, Kuaishou, Ocean Engine AD/Local, Alipay Denghuo) with person-account bindings, project classification, creative workload attribution, and spend/performance/custom calibers."
        },
        "paths": {
            "/api/v1/health": { "get": { "summary": "Kernel, DB, and connector health" } },
            "/api/v1/plugins": { "get": { "summary": "List mounted microkernel plugins, hooks, and services" } },
            "/api/v1/platforms": { "get": { "summary": "List supported platforms and official field mappings" } },
            "/api/v1/platforms/{id}/onboard": { "get": { "summary": "Get official API onboarding steps & opt-in ad-creation warning for a platform" } },
            "/api/v1/platforms/{id}/discover-accounts": { "get": { "summary": "Discover authorized sub-accounts (Tier-1 Official MCP Auth Bridge -> Tier-2 Direct API fallback)" } },
            "/api/v1/governance/relabel": { "post": { "summary": "Tier-1 optimal workflow: re-label 人-户关系 and 项目关系 on authenticated accounts" } },
            "/api/v1/governance/fallback-bootstrap": { "post": { "summary": "Tier-2 fallback workflow: create custom project and link discovered sub-accounts" } },
            "/api/v1/accounts": { "get": { "summary": "List ad accounts" }, "post": { "summary": "Create/update ad account" } },
            "/api/v1/people": { "get": { "summary": "List organization people" }, "post": { "summary": "Create/update person" } },
            "/api/v1/bindings": { "get": { "summary": "List person-account bindings (人-户关系)" }, "post": { "summary": "Create/update person-account binding" } },
            "/api/v1/projects": { "get": { "summary": "List projects and project-account links" }, "post": { "summary": "Create/update project" } },
            "/api/v1/creatives": { "get": { "summary": "List creative assets, platform links, and creator contributions" }, "post": { "summary": "Create/update creative asset" } },
            "/api/v1/calibers": { "get": { "summary": "List standard & custom calibers" }, "post": { "summary": "Define/validate a custom caliber formula" } },
            "/api/v1/analytics/query": { "post": { "summary": "Query unified metrics across dimensions & calibers" } },
            "/api/v1/analytics/reconcile": { "post": { "summary": "Reconcile Spend, Performance, and Custom calibers" } },
            "/api/v1/attribution/creatives": { "post": { "summary": "Creative ROI, creator workload yield, and seeding+boost attribution" } },
            "/api/v1/sync/reports": { "post": { "summary": "Trigger high-concurrency batch report sync" } },
            "/api/v1/sync/funds": { "post": { "summary": "Sync daily cash vs grant wallet statements" } },
            "/api/v1/sync/jobs": { "get": { "summary": "List recent sync jobs" } },
            "/api/v1/ingest": { "post": { "summary": "Ingest external CSV/JSON records (e.g. Pugongying/Huahuo)" } },
            "/api/v1/ad-ops/campaigns": { "get": { "summary": "List cached/live campaigns & adgroups" } },
            "/api/v1/ad-ops/mutate": { "post": { "summary": "Execute guarded ad mutation (defaults dry_run=true)" } },
            "/api/v1/ad-creation/status": { "get": { "summary": "Inspect opt-in Ad Creation (创编) status, ask prompt, risk warnings, and platform capabilities" } },
            "/api/v1/ad-creation/create": { "post": { "summary": "Opt-in Ad Creation (disabled by default with 403 risk disclosure; requires enabled=true, risk_acknowledged=true, and dry_run=true preview)" } },
            "/api/v1/audit-logs": { "get": { "summary": "List immutable mutation & creation audit logs" } },
            "/api/v1/ui/schema": { "get": { "summary": "Schema-driven UI descriptor for custom Web & MiniProgram frontends" } }
        }
    })
}
