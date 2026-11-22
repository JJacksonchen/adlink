//! Core MCP handler, transport exports, and Safety Guardrail kernel plugin.

pub mod auth_flows;
pub mod guardrail;
pub mod http;
pub mod prompts;
pub mod protocol;
pub mod resources;
pub mod stdio;
pub mod tools;

use std::collections::HashSet;
use std::sync::Arc;

use adlink_kernel::Kernel;
use serde_json::{json, Value};

pub use auth_flows::{
    discover_platform_accounts_enriched, fallback_bootstrap_project, relabel_account_governance,
};
pub use guardrail::{
    execute_guarded_ad_creation, execute_guarded_ad_mutation, get_ad_creation_disclosure,
    AdCreationGuardrailError, AdSafetyGuardrailPlugin, GuardrailConfig,
};
pub use http::mcp_http_router;
pub use protocol::{negotiate_protocol_version, JsonRpcRequest, JsonRpcResponse};
pub use stdio::run_stdio_server;

/// Central MCP JSON-RPC dispatcher shared across Stdio, Streamable HTTP, and Legacy SSE.
pub struct McpHandler {
    kernel: Arc<Kernel>,
    enabled_toolsets: Option<HashSet<String>>,
}

impl McpHandler {
    pub fn new(kernel: Arc<Kernel>, enabled_toolsets: Option<HashSet<String>>) -> Self {
        Self {
            kernel,
            enabled_toolsets,
        }
    }

    pub async fn handle_raw_json(
        &self,
        raw: &str,
        session_id: Option<&str>,
    ) -> Option<JsonRpcResponse> {
        let req: JsonRpcRequest = match serde_json::from_str(raw) {
            Ok(r) => r,
            Err(err) => {
                return Some(JsonRpcResponse::err(
                    Value::Null,
                    -32700,
                    format!("invalid JSON-RPC payload: {err}"),
                    None,
                ))
            }
        };
        self.handle_request(req, session_id).await
    }

    pub async fn handle_request(
        &self,
        req: JsonRpcRequest,
        _session_id: Option<&str>,
    ) -> Option<JsonRpcResponse> {
        let Some(id) = req.id.clone() else {
            // JSON-RPC notification (e.g. `notifications/initialized`) produces no response.
            return None;
        };

        let resp = match req.method.as_str() {
            "initialize" => {
                let requested_ver = req
                    .params
                    .as_ref()
                    .and_then(|p| p.get("protocolVersion"))
                    .and_then(Value::as_str);
                let protocol_version = negotiate_protocol_version(requested_ver);
                JsonRpcResponse::ok(
                    id,
                    json!({
                        "protocolVersion": protocol_version,
                        "capabilities": {
                            "tools": { "listChanged": false },
                            "resources": { "subscribe": false, "listChanged": false },
                            "prompts": { "listChanged": false }
                        },
                        "serverInfo": {
                            "name": "adlink-mcp",
                            "version": env!("CARGO_PKG_VERSION")
                        },
                        "instructions": "AdLink connects 15 mainstream domestic CN (10) & global overseas (5: TikTok, Meta, Google, Apple Search Ads, Amazon) ad platforms with multi-currency FX conversion and enterprise governance (人-户关系, 项目分类, 创意归因, 消耗/业绩/自定义口径). Prefer Official MCP auth bridge + governance relabeling (`adlink_relabel_account_governance`), or fallback to `adlink_discover_platform_accounts` + `adlink_fallback_bootstrap_project`. Ad Creation (`创编`) is DISABLED by default: call `adlink_creation_status_and_risks`, ALWAYS ask the user before enabling it, and disclose creation risks."
                    }),
                )
            }
            "ping" => JsonRpcResponse::ok(id, json!({})),
            "tools/list" => {
                let tools = tools::all_tools(self.enabled_toolsets.as_ref());
                JsonRpcResponse::ok(id, json!({ "tools": tools }))
            }
            "tools/call" => {
                let tool_name = req
                    .params
                    .as_ref()
                    .and_then(|p| p.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let arguments = req
                    .params
                    .as_ref()
                    .and_then(|p| p.get("arguments"))
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let result = tools::dispatch_tool(&self.kernel, tool_name, arguments).await;
                JsonRpcResponse::ok(id, result)
            }
            "resources/list" => JsonRpcResponse::ok(id, resources::list_resources()),
            "resources/read" => {
                let uri = req
                    .params
                    .as_ref()
                    .and_then(|p| p.get("uri"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                match resources::read_resource(&self.kernel, uri).await {
                    Ok(res) => JsonRpcResponse::ok(id, res),
                    Err(msg) => JsonRpcResponse::err(id, -32602, msg, None),
                }
            }
            "prompts/list" => JsonRpcResponse::ok(id, prompts::list_prompts()),
            "prompts/get" => {
                let name = req
                    .params
                    .as_ref()
                    .and_then(|p| p.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let arguments = req.params.as_ref().and_then(|p| p.get("arguments"));
                match prompts::get_prompt(name, arguments) {
                    Ok(res) => JsonRpcResponse::ok(id, res),
                    Err(msg) => JsonRpcResponse::err(id, -32602, msg, None),
                }
            }
            other => JsonRpcResponse::err(id, -32601, format!("method not found: {other}"), None),
        };
        Some(resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adlink_connectors::{mount_all_connectors, seed_mock_governance, ConnectorSuiteConfig};
    use adlink_storage::{SqliteStorage, SqliteStoragePlugin, StorageService};
    use axum::body::Body;
    use axum::http::Request;
    use tempfile::tempdir;
    use tower::ServiceExt;

    async fn setup_handler_with_creation(
        creation_cfg: adlink_domain::AdCreationConfig,
    ) -> (tempfile::TempDir, Arc<McpHandler>, Arc<SqliteStorage>) {
        let dir = tempdir().unwrap();
        let storage = Arc::new(SqliteStorage::open(dir.path().join("mcp.db")).unwrap());
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
                    creation_cfg,
                )),
                Value::Null,
            )
            .await
            .unwrap();
        mount_all_connectors(&kernel, &ConnectorSuiteConfig::default())
            .await
            .unwrap();
        seed_mock_governance(storage.as_ref()).await.unwrap();
        (dir, Arc::new(McpHandler::new(kernel, None)), storage)
    }

    async fn setup_handler() -> (tempfile::TempDir, Arc<McpHandler>) {
        let (dir, handler, _storage) =
            setup_handler_with_creation(adlink_domain::AdCreationConfig::default()).await;
        (dir, handler)
    }

    #[tokio::test]
    async fn mcp_tools_guardrail_and_streamable_http_work_end_to_end() {
        let (_dir, handler) = setup_handler().await;
        let app = mcp_http_router(Arc::clone(&handler));

        // 1. Initialize via Streamable HTTP POST /mcp and verify Mcp-Session-Id is minted.
        let init_req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": { "protocolVersion": "2025-06-18" }
                })
                .to_string(),
            ))
            .unwrap();
        let init_resp = app.clone().oneshot(init_req).await.unwrap();
        assert_eq!(init_resp.status(), 200);
        let session_id = init_resp
            .headers()
            .get("Mcp-Session-Id")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        // 2. Call tools/list with session id.
        let list_req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .header("Mcp-Session-Id", &session_id)
            .body(Body::from(
                json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }).to_string(),
            ))
            .unwrap();
        let list_resp = app.clone().oneshot(list_req).await.unwrap();
        assert_eq!(list_resp.status(), 200);
        let list_bytes = axum::body::to_bytes(list_resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let list_json: Value = serde_json::from_slice(&list_bytes).unwrap();
        assert_eq!(list_json["result"]["tools"].as_array().unwrap().len(), 24);

        // 3. Guardrail blocks excessive budget mutation (`999_999_999` fen > 50_000_000 cap).
        let bad_mut = handler
            .handle_raw_json(
                &json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/call",
                    "params": {
                        "name": "adlink_ad_mutate",
                        "arguments": {
                            "platform": "tencent_ads",
                            "ad_account_id": "acc-tencent-01",
                            "entity_level": "ad_group",
                            "entity_ids": ["tc_adg_01"],
                            "action": { "type": "update_budget", "daily_budget_fen": 999999999 },
                            "dry_run": true,
                            "reason": "test cap"
                        }
                    }
                })
                .to_string(),
                None,
            )
            .await
            .unwrap();
        let res = bad_mut.result.unwrap();
        assert_eq!(res["isError"], true);
        assert!(res["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("BUDGET_CAP_EXCEEDED"));

        // 4. Valid dry_run mutation succeeds & audit log captures both rejected and dry_run entries.
        let ok_mut = handler
            .handle_raw_json(
                &json!({
                    "jsonrpc": "2.0",
                    "id": 4,
                    "method": "tools/call",
                    "params": {
                        "name": "adlink_ad_mutate",
                        "arguments": {
                            "platform": "tencent_ads",
                            "ad_account_id": "acc-tencent-01",
                            "entity_level": "ad_group",
                            "entity_ids": ["tc_adg_01"],
                            "action": { "type": "update_budget", "daily_budget_fen": 150000 },
                            "dry_run": true,
                            "reason": "routine scaling preview"
                        }
                    }
                })
                .to_string(),
                None,
            )
            .await
            .unwrap();
        let ok_res = ok_mut.result.unwrap();
        assert_eq!(ok_res["isError"], false);
        assert_eq!(ok_res["structuredContent"]["dry_run"], true);

        let logs_resp = handler
            .handle_raw_json(
                &json!({
                    "jsonrpc": "2.0",
                    "id": 5,
                    "method": "tools/call",
                    "params": { "name": "adlink_audit_logs", "arguments": { "limit": 10 } }
                })
                .to_string(),
                None,
            )
            .await
            .unwrap();
        let logs_arr = logs_resp.result.unwrap()["structuredContent"]["audit_logs"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(logs_arr.len(), 2);

        // 5. Stateless Streamable HTTP call (omitting Mcp-Session-Id) succeeds.
        let stateless_req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "jsonrpc": "2.0",
                    "id": 6,
                    "method": "tools/call",
                    "params": { "name": "adlink_platform_catalog", "arguments": {} }
                })
                .to_string(),
            ))
            .unwrap();
        let stateless_resp = app.clone().oneshot(stateless_req).await.unwrap();
        assert_eq!(stateless_resp.status(), 200);
        let stateless_bytes = axum::body::to_bytes(stateless_resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let stateless_json: Value = serde_json::from_slice(&stateless_bytes).unwrap();
        assert_eq!(
            stateless_json["result"]["structuredContent"]["count"],
            15
        );

        // 6. DELETE /mcp terminates the session and subsequent calls using that ID return 404.
        let del_req = Request::builder()
            .method("DELETE")
            .uri("/mcp")
            .header("Mcp-Session-Id", &session_id)
            .body(Body::empty())
            .unwrap();
        let del_resp = app.clone().oneshot(del_req).await.unwrap();
        assert_eq!(del_resp.status(), 204);

        let stale_req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .header("Mcp-Session-Id", &session_id)
            .body(Body::from(
                json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/list" }).to_string(),
            ))
            .unwrap();
        let stale_resp = app.clone().oneshot(stale_req).await.unwrap();
        assert_eq!(stale_resp.status(), 404);

        // 7. Legacy SSE GET /sse + POST /messages?sessionId=... round-trips JSON-RPC over SSE stream.
        use futures_util::StreamExt;
        let sse_req = Request::builder()
            .method("GET")
            .uri("/sse")
            .body(Body::empty())
            .unwrap();
        let sse_resp = app.clone().oneshot(sse_req).await.unwrap();
        assert_eq!(sse_resp.status(), 200);
        assert!(sse_resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("text/event-stream"));
        let mut data_stream = sse_resp.into_body().into_data_stream();
        let first_chunk = data_stream.next().await.unwrap().unwrap();
        let sse_text = String::from_utf8_lossy(&first_chunk);
        assert!(sse_text.contains("event: endpoint"));
        let msg_path = sse_text
            .lines()
            .find_map(|line| line.strip_prefix("data: ").or_else(|| line.strip_prefix("data:")))
            .unwrap()
            .trim()
            .to_string();
        assert!(msg_path.starts_with("/messages?sessionId="));

        let msg_req = Request::builder()
            .method("POST")
            .uri(&msg_path)
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "jsonrpc": "2.0", "id": 8, "method": "tools/list" }).to_string(),
            ))
            .unwrap();
        let msg_resp = app.clone().oneshot(msg_req).await.unwrap();
        assert_eq!(msg_resp.status(), 202);

        let second_chunk = data_stream.next().await.unwrap().unwrap();
        let msg_event = String::from_utf8_lossy(&second_chunk);
        assert!(msg_event.contains("event: message"));
        assert!(msg_event.contains("adlink_onboard_guide"));

        // 8. Verify MCP resources & prompts endpoints return valid catalog data.
        let res_list = handler
            .handle_raw_json(
                &json!({"jsonrpc":"2.0","id":9,"method":"resources/list"}).to_string(),
                None,
            )
            .await
            .unwrap()
            .result
            .unwrap();
        let uris: Vec<String> = res_list["resources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["uri"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(uris.len(), 4);
        for uri in uris {
            let read = handler
                .handle_raw_json(
                    &json!({"jsonrpc":"2.0","id":10,"method":"resources/read","params":{"uri":uri}}).to_string(),
                    None,
                )
                .await
                .unwrap();
            assert!(read.error.is_none());
            assert!(!read.result.unwrap()["contents"].as_array().unwrap().is_empty());
        }

        let prompt_list = handler
            .handle_raw_json(
                &json!({"jsonrpc":"2.0","id":11,"method":"prompts/list"}).to_string(),
                None,
            )
            .await
            .unwrap()
            .result
            .unwrap();
        assert_eq!(prompt_list["prompts"].as_array().unwrap().len(), 3);
        for pname in ["onboard_ad_platform", "daily_agency_briefing", "creative_workload_review"] {
            let pget = handler
                .handle_raw_json(
                    &json!({"jsonrpc":"2.0","id":12,"method":"prompts/get","params":{"name":pname}}).to_string(),
                    None,
                )
                .await
                .unwrap();
            assert!(pget.error.is_none());
        }
    }

    #[tokio::test]
    async fn ad_creation_opt_in_gate_and_two_tier_login_relabel_work() {
        // A. Default AdCreationConfig blocks ad creation with AD_CREATION_DISABLED_ASK_USER + risk warnings
        let (_dir, default_handler) = setup_handler().await;
        let status_call = default_handler
            .handle_raw_json(
                &json!({
                    "jsonrpc": "2.0",
                    "id": 100,
                    "method": "tools/call",
                    "params": { "name": "adlink_creation_status_and_risks", "arguments": {} }
                })
                .to_string(),
                None,
            )
            .await
            .unwrap();
        let status_sc = &status_call.result.unwrap()["structuredContent"];
        assert_eq!(status_sc["enabled"], false);
        assert_eq!(status_sc["risk_acknowledged"], false);
        assert_eq!(status_sc["risk_warnings"].as_array().unwrap().len(), 5);

        let blocked = default_handler
            .handle_raw_json(
                &json!({
                    "jsonrpc": "2.0",
                    "id": 101,
                    "method": "tools/call",
                    "params": {
                        "name": "adlink_ad_create",
                        "arguments": {
                            "platform": "tencent_ads",
                            "ad_account_id": "acc-tencent-01",
                            "entity_kind": "ad_group",
                            "name": "Blocked Default Create",
                            "daily_budget_fen": 100000,
                            "reason": "should be blocked by default"
                        }
                    }
                })
                .to_string(),
                None,
            )
            .await
            .unwrap()
            .result
            .unwrap();
        assert_eq!(blocked["isError"], true);
        let blocked_txt = blocked["content"][0]["text"].as_str().unwrap();
        assert!(blocked_txt.contains("AD_CREATION_DISABLED_ASK_USER"));
        assert!(blocked_txt.contains("risk_warnings"));

        // B. Partial opt-in without risk_acknowledged is still rejected
        let (_dir2, unack_handler, _st2) = setup_handler_with_creation(adlink_domain::AdCreationConfig {
            enabled: true,
            risk_acknowledged: false,
            ..Default::default()
        })
        .await;
        let unack = unack_handler
            .handle_raw_json(
                &json!({
                    "jsonrpc": "2.0",
                    "id": 102,
                    "method": "tools/call",
                    "params": {
                        "name": "adlink_ad_create",
                        "arguments": {
                            "platform": "tencent_ads",
                            "ad_account_id": "acc-tencent-01",
                            "entity_kind": "ad_group",
                            "name": "Unacked Create",
                            "daily_budget_fen": 100000,
                            "reason": "should require risk acknowledgement"
                        }
                    }
                })
                .to_string(),
                None,
            )
            .await
            .unwrap()
            .result
            .unwrap();
        assert_eq!(unack["isError"], true);
        assert!(unack["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("AD_CREATION_RISK_NOT_ACKNOWLEDGED"));

        // C. Full opt-in: dry_run preview -> enforce preview_confirmed -> live mock creation persists entity!
        let (_dir3, enabled_handler, storage3) =
            setup_handler_with_creation(adlink_domain::AdCreationConfig {
                enabled: true,
                risk_acknowledged: true,
                ..Default::default()
            })
            .await;

        let dry = enabled_handler
            .handle_raw_json(
                &json!({
                    "jsonrpc": "2.0",
                    "id": 103,
                    "method": "tools/call",
                    "params": {
                        "name": "adlink_ad_create",
                        "arguments": {
                            "platform": "kuaishou",
                            "ad_account_id": "acc-kuaishou-01",
                            "entity_kind": "campaign",
                            "name": "OptedIn KS Campaign",
                            "daily_budget_fen": 80000,
                            "dry_run": true,
                            "reason": "preview budget conversion"
                        }
                    }
                })
                .to_string(),
                None,
            )
            .await
            .unwrap()
            .result
            .unwrap();
        assert_eq!(dry["isError"], false);
        assert_eq!(
            dry["structuredContent"]["platform_payload_preview"]["day_budget"],
            800000
        );

        let no_confirm = enabled_handler
            .handle_raw_json(
                &json!({
                    "jsonrpc": "2.0",
                    "id": 104,
                    "method": "tools/call",
                    "params": {
                        "name": "adlink_ad_create",
                        "arguments": {
                            "platform": "kuaishou",
                            "ad_account_id": "acc-kuaishou-01",
                            "entity_kind": "campaign",
                            "name": "No Confirm Live",
                            "daily_budget_fen": 80000,
                            "dry_run": false,
                            "preview_confirmed": false,
                            "reason": "bypass check"
                        }
                    }
                })
                .to_string(),
                None,
            )
            .await
            .unwrap()
            .result
            .unwrap();
        assert_eq!(no_confirm["isError"], true);
        assert!(no_confirm["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("DRY_RUN_PREVIEW_REQUIRED"));

        let confirmed = enabled_handler
            .handle_raw_json(
                &json!({
                    "jsonrpc": "2.0",
                    "id": 105,
                    "method": "tools/call",
                    "params": {
                        "name": "adlink_ad_create",
                        "arguments": {
                            "platform": "kuaishou",
                            "ad_account_id": "acc-kuaishou-01",
                            "entity_kind": "campaign",
                            "name": "Confirmed Mock Campaign",
                            "daily_budget_fen": 80000,
                            "dry_run": false,
                            "preview_confirmed": true,
                            "reason": "user approved creation"
                        }
                    }
                })
                .to_string(),
                None,
            )
            .await
            .unwrap()
            .result
            .unwrap();
        assert_eq!(confirmed["isError"], false);
        let new_id = confirmed["structuredContent"]["created_entity_id"]
            .as_str()
            .unwrap()
            .to_string();
        let saved_entities = storage3
            .list_ad_entities(Some(adlink_domain::PlatformId::Kuaishou), Some("acc-kuaishou-01"))
            .await
            .unwrap();
        assert!(saved_entities.iter().any(|e| e.entity_id == new_id));

        // D. Two-tier Login Architecture: Discover -> Tier-1 Relabel (replaces cleanly) -> Tier-2 Fallback Bootstrap
        let disc = enabled_handler
            .handle_raw_json(
                &json!({
                    "jsonrpc": "2.0",
                    "id": 106,
                    "method": "tools/call",
                    "params": {
                        "name": "adlink_discover_platform_accounts",
                        "arguments": { "platform": "tencent_ads" }
                    }
                })
                .to_string(),
                None,
            )
            .await
            .unwrap()
            .result
            .unwrap();
        assert_eq!(disc["isError"], false);
        assert_eq!(disc["structuredContent"]["discovered_count"], 3);
        assert_eq!(
            disc["structuredContent"]["accounts"][0]["already_registered_ad_account_id"],
            "acc-tencent-01"
        );

        let relabel = enabled_handler
            .handle_raw_json(
                &json!({
                    "jsonrpc": "2.0",
                    "id": 107,
                    "method": "tools/call",
                    "params": {
                        "name": "adlink_relabel_account_governance",
                        "arguments": {
                            "platform": "tencent_ads",
                            "platform_account_id": "38291044",
                            "auth_strategy": "official_mcp_bridge",
                            "official_mcp_endpoint": "https://mcp.example.com/tencent",
                            "replace_existing_bindings": true,
                            "person_bindings": [{
                                "person_id": "per-media-chen",
                                "relation_role": "primary_optimizer",
                                "split_weight": 1.0,
                                "effective_from": "2026-11-01"
                            }],
                            "replace_existing_project_links": true,
                            "project_links": [{
                                "project_id": "prj-ecom-skincare",
                                "allocation_weight": 1.0
                            }]
                        }
                    }
                })
                .to_string(),
                None,
            )
            .await
            .unwrap()
            .result
            .unwrap();
        assert_eq!(relabel["isError"], false, "relabel failed: {relabel}");
        let acct_bindings: Vec<_> = storage3
            .list_person_account_bindings()
            .await
            .unwrap()
            .into_iter()
            .filter(|b| b.ad_account_id == "acc-tencent-01")
            .collect();
        assert_eq!(acct_bindings.len(), 1);
        assert_eq!(acct_bindings[0].person_id, "per-media-chen");

        let bootstrap = enabled_handler
            .handle_raw_json(
                &json!({
                    "jsonrpc": "2.0",
                    "id": 108,
                    "method": "tools/call",
                    "params": {
                        "name": "adlink_fallback_bootstrap_project",
                        "arguments": {
                            "project": {
                                "id": "prj-fallback-new",
                                "org_id": "org-starpulse",
                                "code": "FB-NEW-01",
                                "name": "降级自建项目测试",
                                "client_name": "测试品牌",
                                "brand_name": "TestBrand",
                                "project_type": "ecommerce_gmv",
                                "business_line": "美妆",
                                "settlement_mode": "media_spend_plus_fee",
                                "budget_fen": 5000000,
                                "start_date": "2026-11-01",
                                "tags": ["fallback"]
                            },
                            "accounts": [{
                                "platform": "tencent_ads",
                                "platform_account_id": "3829104401",
                                "account_name": "新拉取子账户A",
                                "owner_person_id": "per-opt-lin"
                            }]
                        }
                    }
                })
                .to_string(),
                None,
            )
            .await
            .unwrap()
            .result
            .unwrap();
        assert_eq!(bootstrap["isError"], false, "bootstrap failed: {bootstrap}");
        let projects = storage3.list_projects().await.unwrap();
        assert!(projects.iter().any(|p| p.id == "prj-fallback-new"));
    }
}
