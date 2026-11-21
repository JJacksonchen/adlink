//! Built-in `AdSafetyGuardrailPlugin` mounted via kernel hooks:
//! - `BeforeAdMutate` / `AfterAdMutate` (budget/bid ceilings, batch limits, audit log)
//! - `BeforeAdCreate` / `AfterAdCreate` (opt-in `创编` gate defaulting to OFF + user-ask & risk disclosure)

use std::sync::Arc;

use adlink_domain::{
    AdCreationConfig, AdCreationRequest, AdCreationResult, AdCreationRiskDisclosure, AdEntity,
    AdMutationAction, AdMutationRequest, AuditLogEntry, Fen,
};
use adlink_kernel::{
    HookDecision, HookHandler, HookPoint, Kernel, KernelError, KernelResult, Plugin,
    PluginCategory, PluginContext, PluginManifest,
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

struct AdCreationSafetyHook {
    config: AdCreationConfig,
}

#[async_trait]
impl HookHandler for AdCreationSafetyHook {
    fn id(&self) -> &str {
        "guardrail.ad_creation_opt_in"
    }

    fn priority(&self) -> i32 {
        5
    }

    async fn handle(&self, point: HookPoint, payload: Value) -> HookDecision {
        if point != HookPoint::BeforeAdCreate {
            return HookDecision::Continue;
        }
        if !self.config.enabled {
            return HookDecision::Reject {
                code: "AD_CREATION_DISABLED_ASK_USER".into(),
                reason: "AdLink 默认关闭广告创编（enabled=false）。请先询问用户『是否需要接入广告创编（新建计划/单元/创意）能力？』并告知创编风险（误烧预算、审核拒审与封户、定向错配、平台单位差异）。用户确认后请在 config/adlink.toml 设置 [ad_creation] enabled = true 与 risk_acknowledged = true。".into(),
            };
        }
        if !self.config.risk_acknowledged {
            return HookDecision::Reject {
                code: "AD_CREATION_RISK_NOT_ACKNOWLEDGED".into(),
                reason: "已开启 [ad_creation].enabled 但尚未确认风险（risk_acknowledged=false）。请向用户展示完整创编风险清单并取得明确确认后再启用。".into(),
            };
        }
        let Ok(req) = serde_json::from_value::<AdCreationRequest>(payload) else {
            return HookDecision::Reject {
                code: "INVALID_CREATION_PAYLOAD".into(),
                reason: "unable to parse AdCreationRequest".into(),
            };
        };
        if !self.config.allowed_platforms.is_empty()
            && !self.config.allowed_platforms.contains(&req.platform)
        {
            return HookDecision::Reject {
                code: "PLATFORM_NOT_ALLOWED_FOR_CREATION".into(),
                reason: format!(
                    "platform '{}' is not in [ad_creation].allowed_platforms allowlist",
                    req.platform
                ),
            };
        }
        if req.name.trim().is_empty() {
            return HookDecision::Reject {
                code: "EMPTY_ENTITY_NAME".into(),
                reason: "ad entity `name` cannot be empty".into(),
            };
        }
        if req.reason.trim().is_empty() {
            return HookDecision::Reject {
                code: "REASON_REQUIRED".into(),
                reason: "an explicit operational `reason` is mandatory for ad creation".into(),
            };
        }
        if let Some(budget) = req.daily_budget_fen {
            if budget <= 0 || budget > self.config.max_initial_daily_budget_fen {
                return HookDecision::Reject {
                    code: "CREATION_BUDGET_CAP_EXCEEDED".into(),
                    reason: format!(
                        "requested initial daily budget {} exceeds safety cap {}",
                        Fen(budget),
                        Fen(self.config.max_initial_daily_budget_fen)
                    ),
                };
            }
        }
        if let Some(bid) = req.bid_fen {
            if bid <= 0 || bid > self.config.max_initial_bid_fen {
                return HookDecision::Reject {
                    code: "CREATION_BID_OUT_OF_RANGE".into(),
                    reason: format!(
                        "requested initial bid {} outside safe bounds (0, {}]",
                        Fen(bid),
                        Fen(self.config.max_initial_bid_fen)
                    ),
                };
            }
        }
        if self.config.require_dry_run_preview && !req.dry_run && !req.preview_confirmed {
            return HookDecision::Reject {
                code: "DRY_RUN_PREVIEW_REQUIRED".into(),
                reason: "require_dry_run_preview=true: please execute dry_run=true first, show platform_payload_preview & risks to the user, and set preview_confirmed=true before live submission.".into(),
            };
        }
        HookDecision::Continue
    }
}

/// Plugin registering [`BudgetSafetyHook`] (`BeforeAdMutate`) and opt-in [`AdCreationSafetyHook`]
/// (`BeforeAdCreate`), while publishing [`AdCreationConfig`] as a queryable kernel service.
pub struct AdSafetyGuardrailPlugin {
    config: GuardrailConfig,
    creation_config: AdCreationConfig,
}

impl AdSafetyGuardrailPlugin {
    pub fn new(config: GuardrailConfig, creation_config: AdCreationConfig) -> Self {
        Self {
            config,
            creation_config,
        }
    }
}

#[async_trait]
impl Plugin for AdSafetyGuardrailPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "guardrail.ad_safety".into(),
            name: "Ad Safety, Opt-In Creation Gate & Budget Cap Guardrail".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Enforces default-off opt-in Ad Creation consent + risk warnings, dry-run preview checks, budget/bid ceilings, and immutable audit logs".into(),
            category: PluginCategory::Guardrail,
            provides: vec!["guardrail:ad_safety".into(), "service:ad_creation_config".into()],
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
        let creation_cfg = self.creation_config.clone();
        ctx.provide_service(Arc::new(creation_cfg.clone()));
        ctx.register_hook(
            HookPoint::BeforeAdMutate,
            Arc::new(BudgetSafetyHook { config: cfg }),
        );
        ctx.register_hook(
            HookPoint::BeforeAdCreate,
            Arc::new(AdCreationSafetyHook {
                config: creation_cfg,
            }),
        );
        Ok(())
    }
}

/// Returns current opt-in Ad Creation (`创编`) policy status, mandatory user prompt, and 5 risk warnings.
pub fn get_ad_creation_disclosure(kernel: &Kernel) -> AdCreationRiskDisclosure {
    let cfg = kernel
        .get_service::<AdCreationConfig>()
        .map(|arc| (*arc).clone())
        .unwrap_or_default();
    AdCreationRiskDisclosure::from_config(&cfg)
}

/// Structured error returned when `BeforeAdCreate` guardrail or validation blocks ad creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdCreationGuardrailError {
    pub code: String,
    pub message: String,
    pub is_opt_in_block: bool,
    pub disclosure: AdCreationRiskDisclosure,
}

impl std::fmt::Display for AdCreationGuardrailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
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

/// Executes a guarded ad creation (`创编`) through `BeforeAdCreate` / `AfterAdCreate` hooks
/// and persists created entities & immutable audit logs.
pub async fn execute_guarded_ad_creation(
    kernel: &Kernel,
    source: &str,
    request: AdCreationRequest,
) -> Result<AdCreationResult, AdCreationGuardrailError> {
    let disclosure = get_ad_creation_disclosure(kernel);
    let storage = kernel
        .require_service::<dyn StorageService>()
        .map_err(|e| AdCreationGuardrailError {
            code: "STORAGE_UNAVAILABLE".into(),
            message: e.to_string(),
            is_opt_in_block: false,
            disclosure: disclosure.clone(),
        })?;

    let payload = serde_json::to_value(&request).map_err(|e| AdCreationGuardrailError {
        code: "SERIALIZE_ERROR".into(),
        message: e.to_string(),
        is_opt_in_block: false,
        disclosure: disclosure.clone(),
    })?;

    let hook_res = kernel.run_hooks(HookPoint::BeforeAdCreate, payload).await;
    let approved_req: AdCreationRequest = match hook_res {
        Ok(val) => serde_json::from_value(val).unwrap_or(request.clone()),
        Err(err) => {
            let (code, message) = match &err {
                KernelError::HookRejected { code, reason, .. } => (code.clone(), reason.clone()),
                other => ("GUARDRAIL_REJECTED".into(), other.to_string()),
            };
            let is_opt_in_block = matches!(
                code.as_str(),
                "AD_CREATION_DISABLED_ASK_USER"
                    | "AD_CREATION_RISK_NOT_ACKNOWLEDGED"
                    | "PLATFORM_NOT_ALLOWED_FOR_CREATION"
            );
            let _ = storage
                .append_audit_log(&AuditLogEntry {
                    id: format!("aud-{}", Uuid::new_v4()),
                    timestamp: Utc::now(),
                    operator: request.operator.clone(),
                    source: source.to_string(),
                    platform: request.platform,
                    ad_account_id: request.ad_account_id.clone(),
                    action: format!("create_{:?}({})", request.entity_kind, request.name),
                    entity_ids: vec![],
                    dry_run: request.dry_run,
                    status: "rejected_by_guardrail".into(),
                    reason: request.reason.clone(),
                    detail: json!({ "code": code, "error": message }),
                })
                .await;
            return Err(AdCreationGuardrailError {
                code,
                message,
                is_opt_in_block,
                disclosure,
            });
        }
    };

    let accounts = storage
        .list_ad_accounts(Some(approved_req.platform))
        .await
        .map_err(|e| AdCreationGuardrailError {
            code: "STORAGE_ERROR".into(),
            message: e.to_string(),
            is_opt_in_block: false,
            disclosure: disclosure.clone(),
        })?;
    let account = accounts
        .into_iter()
        .find(|a| a.id == approved_req.ad_account_id)
        .ok_or_else(|| AdCreationGuardrailError {
            code: "ACCOUNT_NOT_FOUND".into(),
            message: format!("ad_account_id '{}' not found", approved_req.ad_account_id),
            is_opt_in_block: false,
            disclosure: disclosure.clone(),
        })?;

    let connector = kernel
        .get_named_service::<dyn adlink_engine::AdPlatformConnector>(
            approved_req.platform.as_str(),
        )
        .ok_or_else(|| AdCreationGuardrailError {
            code: "CONNECTOR_NOT_MOUNTED".into(),
            message: format!("connector '{}' not mounted", approved_req.platform),
            is_opt_in_block: false,
            disclosure: disclosure.clone(),
        })?;

    let mut result = connector
        .create_ad_entity(&account, &approved_req)
        .await
        .map_err(|e| AdCreationGuardrailError {
            code: "CONNECTOR_CREATE_FAILED".into(),
            message: e.to_string(),
            is_opt_in_block: false,
            disclosure: disclosure.clone(),
        })?;

    if approved_req.dry_run {
        result.guardrail_notes.push(
            "Executed in dry_run=true preview mode; confirm payload with user and pass dry_run=false + preview_confirmed=true to create live."
                .into(),
        );
    } else if result.accepted {
        if let Some(created_id) = &result.created_entity_id {
            let _ = storage
                .upsert_ad_entities(&[AdEntity {
                    platform: approved_req.platform,
                    ad_account_id: approved_req.ad_account_id.clone(),
                    level: approved_req.entity_kind.to_report_level(),
                    entity_id: created_id.clone(),
                    entity_name: approved_req.name.clone(),
                    parent_entity_id: approved_req.parent_entity_id.clone(),
                    status: approved_req.initial_status.clone(),
                    daily_budget_fen: Fen(approved_req.daily_budget_fen.unwrap_or(0)),
                    bid_fen: Fen(approved_req.bid_fen.unwrap_or(0)),
                    updated_at: Utc::now(),
                    raw_extra: result.platform_response.clone(),
                }])
                .await;
        }
    }

    let _ = kernel
        .run_hooks(
            HookPoint::AfterAdCreate,
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
            action: format!("create_{:?}({})", approved_req.entity_kind, approved_req.name),
            entity_ids: result.created_entity_id.iter().cloned().collect(),
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
