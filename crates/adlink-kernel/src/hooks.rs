//! Interception hook points and priority-ordered pipeline execution.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{KernelError, KernelResult};

/// Lifecycle hook points emitted across AdLink's request, normalization, and mutation flows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookPoint {
    /// Fired immediately before a connector issues an outbound platform API request.
    BeforeApiRequest,
    /// Fired after receiving a raw platform API response payload.
    AfterApiResponse,
    /// Fired after a connector normalizes a raw record into `UnifiedReportRow`.
    AfterNormalizeRow,
    /// Fired before executing an ad mutation (`Pause`, `Resume`, `UpdateBudget`, `UpdateBid`).
    BeforeAdMutate,
    /// Fired after executing an ad mutation (useful for immutable audit logs & webhooks).
    AfterAdMutate,
    /// Fired during caliber evaluation to allow plugins to inject custom derived metrics.
    EnrichMetrics,
}

impl fmt::Display for HookPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::BeforeApiRequest => "before_api_request",
            Self::AfterApiResponse => "after_api_response",
            Self::AfterNormalizeRow => "after_normalize_row",
            Self::BeforeAdMutate => "before_ad_mutate",
            Self::AfterAdMutate => "after_ad_mutate",
            Self::EnrichMetrics => "enrich_metrics",
        };
        f.write_str(name)
    }
}

/// Decision returned by a [`HookHandler`].
#[derive(Debug, Clone)]
pub enum HookDecision {
    /// Leave the payload untouched and continue the pipeline.
    Continue,
    /// Replace the payload for downstream hooks and the caller.
    Modified(Value),
    /// Short-circuit the pipeline and reject the operation with a structured error.
    Reject { code: String, reason: String },
}

/// Pluggable interceptor invoked sequentially by priority at a [`HookPoint`].
#[async_trait]
pub trait HookHandler: Send + Sync {
    /// Stable identifier of this hook handler (used for logging and error attribution).
    fn id(&self) -> &str;

    /// Execution priority: lower numbers run first (e.g. `10` runs before `100`).
    fn priority(&self) -> i32 {
        100
    }

    /// Evaluates or transforms `payload` at the hook point.
    async fn handle(&self, point: HookPoint, payload: Value) -> HookDecision;
}

#[derive(Clone)]
pub(crate) struct RegisteredHook {
    pub plugin_id: String,
    pub registration_id: u64,
    pub handler: Arc<dyn HookHandler>,
}

/// Thread-safe registry managing hook handlers grouped by [`HookPoint`].
#[derive(Default)]
pub struct HookRegistry {
    hooks: DashMap<HookPoint, Vec<RegisteredHook>>,
}

impl HookRegistry {
    /// Creates an empty hook registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a hook handler owned by `plugin_id` and returns a unique registration id.
    pub(crate) fn insert(
        &self,
        plugin_id: &str,
        registration_id: u64,
        point: HookPoint,
        handler: Arc<dyn HookHandler>,
    ) {
        let mut entry = self.hooks.entry(point).or_default();
        entry.push(RegisteredHook {
            plugin_id: plugin_id.to_string(),
            registration_id,
            handler,
        });
        entry.sort_by_key(|item| item.handler.priority());
    }

    /// Removes a specific hook registration during plugin rollback or unload.
    pub(crate) fn remove(&self, point: HookPoint, registration_id: u64) {
        if let Some(mut list) = self.hooks.get_mut(&point) {
            list.retain(|item| item.registration_id != registration_id);
        }
    }

    /// Returns the total count of registered hooks across all hook points.
    pub fn total_count(&self) -> usize {
        self.hooks.iter().map(|entry| entry.value().len()).sum()
    }

    /// Returns true if at least one hook handler is registered at `point`.
    pub fn has_hooks(&self, point: HookPoint) -> bool {
        self.hooks
            .get(&point)
            .is_some_and(|list| !list.is_empty())
    }

    /// Executes all handlers registered for `point` in ascending priority order.
    pub async fn run(&self, point: HookPoint, initial_payload: Value) -> KernelResult<Value> {
        let handlers: Vec<Arc<dyn HookHandler>> = match self.hooks.get(&point) {
            Some(list) => list.iter().map(|r| Arc::clone(&r.handler)).collect(),
            None => return Ok(initial_payload),
        };

        let mut current = initial_payload;
        for handler in handlers {
            match handler.handle(point, current.clone()).await {
                HookDecision::Continue => {}
                HookDecision::Modified(next) => {
                    current = next;
                }
                HookDecision::Reject { code, reason } => {
                    return Err(KernelError::HookRejected {
                        hook_id: handler.id().to_string(),
                        point: point.to_string(),
                        code,
                        reason,
                    });
                }
            }
        }
        Ok(current)
    }

    /// Lists registered hook metadata for observability and diagnostics.
    pub fn list(&self) -> Vec<(HookPoint, String, String, i32)> {
        let mut out = Vec::new();
        for entry in self.hooks.iter() {
            let point = *entry.key();
            for item in entry.value() {
                out.push((
                    point,
                    item.plugin_id.clone(),
                    item.handler.id().to_string(),
                    item.handler.priority(),
                ));
            }
        }
        out.sort_by_key(|(_, _, _, priority)| *priority);
        out
    }
}
