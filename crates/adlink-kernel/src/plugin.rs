//! Plugin trait, manifest metadata, and runtime health status types.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::context::PluginContext;
use crate::error::KernelResult;

/// Architectural category of a mounted plugin.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginCategory {
    /// Ad platform API connector (e.g. Tencent Ads, Ocean Engine, XHS Juguang).
    Connector,
    /// Persistence backend (e.g. SQLite WAL, PostgreSQL, ClickHouse).
    Storage,
    /// Metric caliber evaluator or formula extension.
    MetricCaliber,
    /// Creative/creator/account attribution engine.
    Attribution,
    /// Organization, person-account, or project governance module.
    Governance,
    /// MCP tool/resource/prompt pack.
    McpPack,
    /// Safety guardrail / budget cap / audit interceptor.
    Guardrail,
    /// Transport adapter (Stdio, Streamable HTTP, SSE, REST).
    Transport,
    /// User-defined third-party extension category.
    Custom(String),
}

/// Static declarative manifest describing a plugin's identity, capabilities, and dependencies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Globally unique plugin identifier (e.g. `connector.tencent_ads`).
    pub id: String,
    /// Human-friendly display name.
    pub name: String,
    /// Semantic version of the plugin.
    pub version: String,
    /// Concise description of what the plugin provides.
    pub description: String,
    /// Architectural category.
    pub category: PluginCategory,
    /// Capability tokens or service tags provided when mounted.
    #[serde(default)]
    pub provides: Vec<String>,
    /// Plugin IDs or capability tokens that must already be mounted before mounting this plugin.
    #[serde(default)]
    pub requires: Vec<String>,
    /// Optional JSON Schema describing valid plugin configuration.
    #[serde(default = "default_schema")]
    pub config_schema: Value,
}

fn default_schema() -> Value {
    serde_json::json!({ "type": "object" })
}

/// Health report returned by a plugin or aggregated across the kernel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginHealth {
    /// Whether the plugin is healthy enough to serve production traffic.
    pub healthy: bool,
    /// Short machine-readable status (`ok`, `degraded`, `unhealthy`).
    pub status: String,
    /// Optional explanatory message or diagnostics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Timestamp when the check was evaluated.
    pub checked_at: DateTime<Utc>,
}

impl PluginHealth {
    /// Constructs a healthy status report.
    pub fn healthy() -> Self {
        Self {
            healthy: true,
            status: "ok".to_string(),
            detail: None,
            checked_at: Utc::now(),
        }
    }

    /// Constructs a degraded status report (operational with warnings).
    pub fn degraded(detail: impl Into<String>) -> Self {
        Self {
            healthy: true,
            status: "degraded".to_string(),
            detail: Some(detail.into()),
            checked_at: Utc::now(),
        }
    }

    /// Constructs an unhealthy status report.
    pub fn unhealthy(detail: impl Into<String>) -> Self {
        Self {
            healthy: false,
            status: "unhealthy".to_string(),
            detail: Some(detail.into()),
            checked_at: Utc::now(),
        }
    }
}

/// Core Cordis-inspired plugin trait: everything in AdLink is a composable plugin.
#[async_trait]
pub trait Plugin: Send + Sync {
    /// Returns immutable metadata describing this plugin.
    fn manifest(&self) -> PluginManifest;

    /// Mounts the plugin into the microkernel using a scoped [`PluginContext`].
    ///
    /// Every service, hook, event subscription, or disposer registered through `ctx` is bound to
    /// this plugin's `EffectScope` and automatically rolled back if `mount` fails or `unload` runs.
    async fn mount(&self, ctx: &mut PluginContext) -> KernelResult<()>;

    /// Optional explicit teardown hook invoked before effect disposers execute during unload.
    async fn unmount(&self) -> KernelResult<()> {
        Ok(())
    }

    /// Evaluates live health of this plugin.
    async fn health(&self) -> PluginHealth {
        PluginHealth::healthy()
    }
}

/// Runtime status snapshot for a mounted plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginStatus {
    pub manifest: PluginManifest,
    pub mounted_at: DateTime<Utc>,
    pub effect_count: usize,
    pub health: PluginHealth,
}

/// Aggregated kernel-wide health report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelHealthReport {
    pub healthy: bool,
    pub plugin_count: usize,
    pub plugins: Vec<PluginStatus>,
}
