//! JSON-RPC 2.0 & MCP (Model Context Protocol) message types and version negotiation.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &[
    "2025-11-25",
    "2025-06-18",
    "2025-03-26",
    "2024-11-05",
];

pub const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";

pub fn negotiate_protocol_version(requested: Option<&str>) -> String {
    if let Some(req) = requested {
        if SUPPORTED_PROTOCOL_VERSIONS.contains(&req) {
            return req.to_string();
        }
    }
    DEFAULT_PROTOCOL_VERSION.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResponse {
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: Value, code: i64, message: impl Into<String>, data: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data,
            }),
        }
    }
}

/// Wraps a tool result into MCP `content: [{ type: "text", text }]` plus `structuredContent`
/// so both legacy text-only agents and modern structured-content agents work cleanly.
pub fn mcp_tool_ok(value: Value) -> Value {
    let pretty = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    json!({
        "content": [
            {
                "type": "text",
                "text": pretty
            }
        ],
        "structuredContent": value,
        "isError": false
    })
}

pub fn mcp_tool_err(message: impl Into<String>) -> Value {
    let msg = message.into();
    json!({
        "content": [
            {
                "type": "text",
                "text": msg
            }
        ],
        "isError": true
    })
}
