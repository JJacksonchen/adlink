//! Core MCP handler, transport exports, and Safety Guardrail kernel plugin.

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

pub use guardrail::{execute_guarded_ad_mutation, AdSafetyGuardrailPlugin, GuardrailConfig};
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
                        "instructions": "AdLink connects 8 mainstream Chinese ad platforms (10 connectors) with enterprise governance (人-户关系, 项目分类, 创意归因, 消耗/业绩/自定义口径). Call adlink_onboard_guide to help the user connect live platform credentials, or query/sync in mock mode immediately."
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
    use adlink_storage::{SqliteStorage, SqliteStoragePlugin};
    use axum::body::Body;
    use axum::http::Request;
    use tempfile::tempdir;
    use tower::ServiceExt;

    async fn setup_handler() -> (tempfile::TempDir, Arc<McpHandler>) {
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
                Arc::new(AdSafetyGuardrailPlugin::new(GuardrailConfig::default())),
                Value::Null,
            )
            .await
            .unwrap();
        mount_all_connectors(&kernel, &ConnectorSuiteConfig::default())
            .await
            .unwrap();
        seed_mock_governance(storage.as_ref()).await.unwrap();
        (dir, Arc::new(McpHandler::new(kernel, None)))
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
        assert_eq!(list_json["result"]["tools"].as_array().unwrap().len(), 19);

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
            10
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
}
