//! Axum HTTP router providing both modern Streamable HTTP (`/mcp`) and Legacy SSE (`/sse` + `/messages`) MCP transports.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use dashmap::DashMap;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use uuid::Uuid;

use crate::protocol::{negotiate_protocol_version, JsonRpcRequest, JsonRpcResponse};
use crate::McpHandler;

#[derive(Clone)]
pub struct HttpMcpState {
    pub handler: Arc<McpHandler>,
    /// Active Streamable HTTP sessions (`session_id -> negotiated protocol version`).
    pub streamable_sessions: Arc<DashMap<String, String>>,
    /// Active Legacy SSE channels (`session_id -> mpsc sender`).
    pub sse_sessions: Arc<DashMap<String, mpsc::Sender<String>>>,
}

impl HttpMcpState {
    pub fn new(handler: Arc<McpHandler>) -> Self {
        Self {
            handler,
            streamable_sessions: Arc::new(DashMap::new()),
            sse_sessions: Arc::new(DashMap::new()),
        }
    }
}

/// Returns an Axum sub-router exposing `/mcp`, `/sse`, and `/messages`.
pub fn mcp_http_router(handler: Arc<McpHandler>) -> Router {
    let state = HttpMcpState::new(handler);
    Router::new()
        .route(
            "/mcp",
            post(handle_streamable_post)
                .get(handle_streamable_get)
                .delete(handle_streamable_delete),
        )
        .route("/sse", get(handle_legacy_sse))
        .route("/messages", post(handle_legacy_messages))
        .with_state(state)
}

async fn handle_streamable_post(
    State(state): State<HttpMcpState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let req: JsonRpcRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(err) => {
            let err_resp = JsonRpcResponse::err(
                serde_json::Value::Null,
                -32700,
                format!("parse error: {err}"),
                None,
            );
            return (
                StatusCode::BAD_REQUEST,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::to_string(&err_resp).unwrap_or_default(),
            )
                .into_response();
        }
    };

    let incoming_session = headers
        .get("Mcp-Session-Id")
        .or_else(|| headers.get("mcp-session-id"))
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let wants_sse = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.contains("text/event-stream") && !s.contains("application/json"))
        .unwrap_or(false);

    // On `initialize`, mint a fresh session ID so stateful clients can echo `Mcp-Session-Id`.
    // If a non-initialize request sends an explicit Mcp-Session-Id that does not exist (or was deleted),
    // return 404 per Streamable HTTP spec; stateless 2026 clients omitting Mcp-Session-Id are still accepted.
    let mut new_session_header: Option<String> = None;
    if req.method == "initialize" {
        let sid = Uuid::new_v4().to_string();
        let req_proto = req
            .params
            .as_ref()
            .and_then(|p| p.get("protocolVersion"))
            .and_then(|v| v.as_str());
        let negotiated = negotiate_protocol_version(req_proto);
        state.streamable_sessions.insert(sid.clone(), negotiated);
        new_session_header = Some(sid);
    } else if let Some(sid) = incoming_session.as_deref() {
        if !state.streamable_sessions.contains_key(sid) {
            return (StatusCode::NOT_FOUND, "unknown or expired Mcp-Session-Id").into_response();
        }
    }

    let resp_opt = state
        .handler
        .handle_request(req, incoming_session.as_deref())
        .await;

    let Some(resp) = resp_opt else {
        let mut res = StatusCode::ACCEPTED.into_response();
        if let Some(sid) = new_session_header {
            if let Ok(hv) = HeaderValue::from_str(&sid) {
                res.headers_mut().insert("Mcp-Session-Id", hv);
            }
        }
        return res;
    };

    let json_text = serde_json::to_string(&resp).unwrap_or_default();
    if wants_sse {
        let stream = tokio_stream::once(Ok::<Event, Infallible>(
            Event::default().event("message").data(json_text),
        ));
        let mut res = Sse::new(stream).into_response();
        if let Some(sid) = new_session_header {
            if let Ok(hv) = HeaderValue::from_str(&sid) {
                res.headers_mut().insert("Mcp-Session-Id", hv);
            }
        }
        return res;
    }

    let mut res = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        json_text,
    )
        .into_response();
    if let Some(sid) = new_session_header {
        if let Ok(hv) = HeaderValue::from_str(&sid) {
            res.headers_mut().insert("Mcp-Session-Id", hv);
        }
    }
    res
}

async fn handle_streamable_get() -> Response {
    // Keepalive SSE stream for server-to-client notifications on `/mcp`.
    let stream = tokio_stream::wrappers::IntervalStream::new(tokio::time::interval(
        Duration::from_secs(15),
    ))
    .map(|_| Ok::<Event, Infallible>(Event::default().comment("adlink-mcp-keepalive")));

    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

async fn handle_streamable_delete(
    State(state): State<HttpMcpState>,
    headers: HeaderMap,
) -> StatusCode {
    if let Some(sid) = headers
        .get("Mcp-Session-Id")
        .or_else(|| headers.get("mcp-session-id"))
        .and_then(|v| v.to_str().ok())
    {
        state.streamable_sessions.remove(sid);
    }
    StatusCode::NO_CONTENT
}

#[derive(Deserialize)]
struct LegacyQuery {
    #[serde(rename = "sessionId")]
    session_id: String,
}

async fn handle_legacy_sse(State(state): State<HttpMcpState>) -> Response {
    let session_id = Uuid::new_v4().to_string();
    let (tx, rx) = mpsc::channel::<String>(64);
    state.sse_sessions.insert(session_id.clone(), tx);

    let endpoint_evt = Event::default()
        .event("endpoint")
        .data(format!("/messages?sessionId={session_id}"));

    let initial = tokio_stream::once(Ok::<Event, Infallible>(endpoint_evt));
    let messages = ReceiverStream::new(rx)
        .map(|payload| Ok::<Event, Infallible>(Event::default().event("message").data(payload)));

    let combined = initial.chain(messages);
    Sse::new(combined)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

async fn handle_legacy_messages(
    State(state): State<HttpMcpState>,
    Query(q): Query<LegacyQuery>,
    body: String,
) -> Response {
    let Some(sender) = state
        .sse_sessions
        .get(&q.session_id)
        .map(|entry| entry.value().clone())
    else {
        return (StatusCode::NOT_FOUND, "unknown SSE sessionId").into_response();
    };

    if let Some(resp) = state
        .handler
        .handle_raw_json(&body, Some(&q.session_id))
        .await
    {
        let encoded = serde_json::to_string(&resp).unwrap_or_default();
        if sender.send(encoded).await.is_err() {
            state.sse_sessions.remove(&q.session_id);
            return (StatusCode::GONE, "SSE stream closed").into_response();
        }
    }

    StatusCode::ACCEPTED.into_response()
}
