//! Built-in `AdSafetyGuardrailPlugin` mounted via kernel hooks (`BeforeAdMutate` / `AfterAdMutate`).

use std::sync::Arc;

use adlink_domain::{AdMutationAction, AdMutationRequest, AuditLogEntry, Fen};
use adlink_kernel::{
    HookDecision, HookHandler, HookPoint, Kernel, KernelResult, Plugin, PluginCategory,
    PluginContext, PluginManifest,
};
use adlink_storage::StorageService;
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardrailConfig {
    /// Hard daily budget ceiling in Fen (default: 500,000 CNY = 50_000_000 fen).
    #[serde(default = "default_max_budget")]
    pub max_daily_budget_fen: i64,
    /// Hard single-entity bid ceiling in Fen (default: 5,000 CNY = 500_000 fen).
    #[serde(default = "default_max_bid")]
    pub max_bid_fen: i64,
    /// Maximum number of entities mutated in one request.
    #[serde(default = "default_max_batch")]
    pub max_batch_size: usize,
}

fn default_max_budget() -> i64 {
    50_000_000
}
fn default_max_bid() -> i64 {
    500_000
}
fn default_max_batch() -> usize {
    20
}

impl Default for GuardrailConfig {
    fn default() -> Self {
        Self {
            max_daily_budget_fen: default_max_budget(),
            max_bid_fen: default_max_bid(),
            max_batch_size: default_max_batch(),
        }
    }
}

struct BudgetSafetyHook {
    config: GuardrailConfig,
}

#[async_trait]
impl HookHandler for BudgetSafetyHook {
    fn id(&self) -> &str {
        "guardrail.ad_safety"
    }

    fn priority(&self) -> i32 {
        10
    }

    async fn handle(&self, point: HookPoint, payload: Value) -> HookDecision {
        if point != HookPoint::BeforeAdMutate {
            return HookDecision::Continue;
        }
        let Ok(req) = serde_json::from_value::<AdMutationRequest>(payload.clone()) else {
            return HookDecision::Reject {
                code: "INVALID_MUTATION_PAYLOAD".into(),
                reason: "unable to parse AdMutationRequest".into(),
            };
        };
        if req.entity_ids.is_empty() {
            return HookDecision::Reject {
                code: "EMPTY_ENTITY_IDS".into(),
                reason: "entity_ids cannot be empty".into(),
            };
        }
        if req.entity_ids.len() > self.config.max_batch_size {
            return HookDecision::Reject {
                code: "BATCH_LIMIT_EXCEEDED".into(),
                reason: format!(
                    "batch size {} exceeds max allowed {}",
                    req.entity_ids.len(),
                    self.config.max_batch_size
                ),
            };
        }
        if req.reason.trim().is_empty() {
            return HookDecision::Reject {
                code: "REASON_REQUIRED".into(),
                reason: "an explicit operational reason is mandatory for ad mutations".into(),
            };
        }
        match req.action {
            AdMutationAction::UpdateBudget { daily_budget_fen } => {
                if daily_budget_fen <= 0 {
                    return HookDecision::Reject {
                        code: "INVALID_BUDGET".into(),
                        reason: "daily_budget_fen must be positive".into(),
                    };
                }
                if daily_budget_fen > self.config.max_daily_budget_fen {
                    return HookDecision::Reject {
                        code: "BUDGET_CAP_EXCEEDED".into(),
                        reason: format!(
                            "requested daily budget {} exceeds safety cap {}",
                            Fen(daily_budget_fen),
                            Fen(self.config.max_daily_budget_fen)
                        ),
                    };
                }
            }
            AdMutationAction::UpdateBid { bid_fen } => {
                if bid_fen <= 0 || bid_fen > self.config.max_bid_fen {
                    return HookDecision::Reject {
                        code: "BID_OUT_OF_RANGE".into(),
                        reason: format!(
                            "requested bid {} outside safe bounds (0, {}]",
                            Fen(bid_fen),
                            Fen(self.config.max_bid_fen)
                        ),
                    };
                }
            }
            _ => {}
        }
        HookDecision::Continue
    }
}

/// Plugin registering [`BudgetSafetyHook`] into `BeforeAdMutate`.
pub struct AdSafetyGuardrailPlugin {
    config: GuardrailConfig,
}

impl AdSafetyGuardrailPlugin {
    pub fn new(config: GuardrailConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Plugin for AdSafetyGuardrailPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "guardrail.ad_safety".into(),
            name: "Ad Safety & Budget Cap Guardrail".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Validates budget/bid ceilings, batch limits, and mandatory audit reasons before ad mutations".into(),
            category: PluginCategory::Guardrail,
            provides: vec!["guardrail:ad_safety".into()],
            requires: vec!["storage".into()],
            config_schema: json!({"type": "object"}),
        }
    }

    async fn mount(&self, ctx: &mut PluginContext) -> KernelResult<()> {
        let cfg: GuardrailConfig = if ctx.config().is_null() {
            self.config.clone()
        } else {
            ctx.config_as().unwrap_or_else(|_| self.config.clone())
        };
        ctx.register_hook(
            HookPoint::BeforeAdMutate,
            Arc::new(BudgetSafetyHook { config: cfg }),
        );
        Ok(())
    }
}

/// Executes a guarded ad mutation through the kernel hook pipeline and logs an immutable audit entry.
pub async fn execute_guarded_ad_mutation(
    kernel: &Kernel,
    source: &str,
    request: AdMutationRequest,
) -> Result<adlink_domain::AdMutationResult, String> {
    let storage = kernel
        .require_service::<dyn StorageService>()
        .map_err(|e| e.to_string())?;

    let payload = serde_json::to_value(&request).map_err(|e| e.to_string())?;
    let hook_res = kernel.run_hooks(HookPoint::BeforeAdMutate, payload).await;
    let approved_req: AdMutationRequest = match hook_res {
        Ok(val) => serde_json::from_value(val).unwrap_or(request.clone()),
        Err(err) => {
            let _ = storage
                .append_audit_log(&AuditLogEntry {
                    id: format!("aud-{}", Uuid::new_v4()),
                    timestamp: Utc::now(),
                    operator: request.operator.clone(),
                    source: source.to_string(),
                    platform: request.platform,
                    ad_account_id: request.ad_account_id.clone(),
                    action: format!("{:?}", request.action),
                    entity_ids: request.entity_ids.clone(),
                    dry_run: request.dry_run,
                    status: "rejected_by_guardrail".into(),
                    reason: request.reason.clone(),
                    detail: json!({ "error": err.to_string() }),
                })
                .await;
            return Err(err.to_string());
        }
    };

    let accounts = storage
        .list_ad_accounts(Some(approved_req.platform))
        .await
        .map_err(|e| e.to_string())?;
    let account = accounts
        .into_iter()
        .find(|a| a.id == approved_req.ad_account_id)
        .ok_or_else(|| format!("ad_account_id '{}' not found", approved_req.ad_account_id))?;

    let connector = kernel
        .get_named_service::<dyn adlink_engine::AdPlatformConnector>(
            approved_req.platform.as_str(),
        )
        .ok_or_else(|| format!("connector '{}' not mounted", approved_req.platform))?;

    let mut result = connector
        .mutate_ad_entities(&account, &approved_req)
        .await
        .map_err(|e| e.to_string())?;
    if approved_req.dry_run {
        result
            .guardrail_notes
            .push("Executed in dry_run=true preview mode; set dry_run=false to apply live.".into());
    }

    let _ = kernel
        .run_hooks(
            HookPoint::AfterAdMutate,
            serde_json::to_value(&result).unwrap_or(Value::Null),
        )
        .await;

    let _ = storage
        .append_audit_log(&AuditLogEntry {
            id: format!("aud-{}", Uuid::new_v4()),
            timestamp: Utc::now(),
            operator: approved_req.operator.clone(),
            source: source.to_string(),
            platform: approved_req.platform,
            ad_account_id: approved_req.ad_account_id.clone(),
            action: format!("{:?}", approved_req.action),
            entity_ids: approved_req.entity_ids.clone(),
            dry_run: approved_req.dry_run,
            status: if approved_req.dry_run {
                "dry_run_passed"
            } else {
                "executed"
            }
            .into(),
            reason: approved_req.reason.clone(),
            detail: serde_json::to_value(&result).unwrap_or(Value::Null),
        })
        .await;

    Ok(result)
}
