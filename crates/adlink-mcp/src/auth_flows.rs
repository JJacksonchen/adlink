//! Shared Two-Tier Login / Account Discovery & Internal Governance Re-Labeling workflows.
//!
//! Used uniformly by both MCP tools and REST API endpoints:
//! - **Tier 1 (Optimal)**: Discover accounts via Official Platform MCP Bridge and re-label (`重标`)
//!   enterprise `人-户关系` (`PersonAccountBinding`) and `项目关系` (`ProjectAccountLink`).
//! - **Tier 2 (Fallback)**: Discover accounts via Direct Marketing API and bootstrap a new custom
//!   project linked directly to selected sub-accounts (`FallbackBootstrapProjectRequest`).

use std::collections::{BTreeMap, HashMap};

use adlink_domain::{
    AccountType, AdAccount, AuditLogEntry, AuthLoginStrategy, BindingRole,
    FallbackBootstrapProjectRequest, PersonAccountBinding, PlatformId, ProjectAccountLink,
    RelabelAccountGovernanceRequest,
};
use adlink_engine::AdPlatformConnector;
use adlink_kernel::Kernel;
use adlink_storage::StorageService;
use chrono::Utc;
use serde_json::{json, Value};
use uuid::Uuid;

/// Discovers platform accounts using Tier-1 Official MCP Bridge (optimal) or Tier-2 Direct API (fallback),
/// enriched with existing AdLink `ad_account_id`, bound people, and linked projects.
pub async fn discover_platform_accounts_enriched(
    kernel: &Kernel,
    platform: PlatformId,
) -> Result<Value, String> {
    let storage = kernel
        .require_service::<dyn StorageService>()
        .map_err(|e| e.to_string())?;
    let connector = kernel
        .get_named_service::<dyn AdPlatformConnector>(platform.as_str())
        .ok_or_else(|| format!("connector '{}' is not mounted", platform.as_str()))?;

    let mut discovered = connector
        .discover_accounts()
        .await
        .map_err(|e| e.to_string())?;

    let existing_accounts = storage
        .list_ad_accounts(Some(platform))
        .await
        .map_err(|e| e.to_string())?;
    let all_bindings = storage
        .list_person_account_bindings()
        .await
        .map_err(|e| e.to_string())?;
    let all_project_links = storage
        .list_project_account_links()
        .await
        .map_err(|e| e.to_string())?;

    let acct_by_platform_id: HashMap<String, AdAccount> = existing_accounts
        .into_iter()
        .map(|a| (a.platform_account_id.clone(), a))
        .collect();
    let mut persons_by_acct: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for b in all_bindings {
        persons_by_acct
            .entry(b.ad_account_id)
            .or_default()
            .push(b.person_id);
    }
    let mut projects_by_acct: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for l in all_project_links {
        projects_by_acct
            .entry(l.ad_account_id)
            .or_default()
            .push(l.project_id);
    }

    for item in &mut discovered {
        if let Some(existing) = acct_by_platform_id.get(&item.platform_account_id) {
            item.already_registered_ad_account_id = Some(existing.id.clone());
            item.current_person_ids = persons_by_acct
                .get(&existing.id)
                .cloned()
                .unwrap_or_default();
            item.current_project_ids = projects_by_acct
                .get(&existing.id)
                .cloned()
                .unwrap_or_default();
        }
    }

    Ok(json!({
        "platform": platform,
        "connector_mode": connector.mode_label(),
        "discovered_count": discovered.len(),
        "accounts": discovered,
        "recommended_workflows": {
            "optimal_tier1": "已通过官方 MCP 登录鉴权的用户，可调用 `adlink_relabel_account_governance` (POST /api/v1/governance/relabel) 自由重标人-户关系与项目归属，无需重建鉴权。",
            "fallback_tier2": "若走降级 API 直连拉取账户列表，可调用 `adlink_fallback_bootstrap_project` (POST /api/v1/governance/fallback-bootstrap) 自建项目并一键关联选中子账户与负责优化师。"
        }
    }))
}

/// Optimal Tier-1 workflow: registers/updates an account authenticated via Official MCP Bridge (or API)
/// and cleanly re-labels (`重标`) enterprise `人-户关系` and `项目关系`.
pub async fn relabel_account_governance(
    kernel: &Kernel,
    source: &str,
    req: RelabelAccountGovernanceRequest,
) -> Result<Value, String> {
    let storage = kernel
        .require_service::<dyn StorageService>()
        .map_err(|e| e.to_string())?;

    let existing_list = storage
        .list_ad_accounts(Some(req.platform))
        .await
        .map_err(|e| e.to_string())?;
    let existing = existing_list.into_iter().find(|a| {
        req.ad_account_id
            .as_deref()
            .is_some_and(|id| id == a.id)
            || a.platform_account_id == req.platform_account_id
    });

    let ad_account_id = req
        .ad_account_id
        .clone()
        .or_else(|| existing.as_ref().map(|a| a.id.clone()))
        .unwrap_or_else(|| format!("acc-{}-{}", req.platform.as_str(), req.platform_account_id));

    let mut extra = existing
        .as_ref()
        .map(|a| a.extra.clone())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    if let Some(obj) = extra.as_object_mut() {
        obj.insert(
            "auth_strategy".into(),
            serde_json::to_value(req.auth_strategy).unwrap_or(json!("official_mcp_bridge")),
        );
        if let Some(ep) = &req.official_mcp_endpoint {
            obj.insert("official_mcp_endpoint".into(), json!(ep));
        }
    }

    let account = AdAccount {
        id: ad_account_id.clone(),
        org_id: req
            .org_id
            .clone()
            .or_else(|| existing.as_ref().map(|a| a.org_id.clone()))
            .unwrap_or_else(|| "org-starpulse".into()),
        platform: req.platform,
        platform_account_id: req.platform_account_id.clone(),
        account_name: req
            .account_name
            .clone()
            .or_else(|| existing.as_ref().map(|a| a.account_name.clone()))
            .unwrap_or_else(|| {
                format!(
                    "{} - {}",
                    req.platform.display_name_zh(),
                    req.platform_account_id
                )
            }),
        account_type: req
            .account_type
            .clone()
            .or_else(|| existing.as_ref().map(|a| a.account_type.clone()))
            .unwrap_or(AccountType::AgencySub),
        rebate_rate: req
            .rebate_rate
            .or_else(|| existing.as_ref().map(|a| a.rebate_rate))
            .unwrap_or(0.05),
        service_fee_rate: req
            .service_fee_rate
            .or_else(|| existing.as_ref().map(|a| a.service_fee_rate))
            .unwrap_or(0.08),
        default_cash_ratio: req
            .default_cash_ratio
            .or_else(|| existing.as_ref().map(|a| a.default_cash_ratio))
            .unwrap_or(0.9),
        status: existing
            .as_ref()
            .map(|a| a.status.clone())
            .unwrap_or_else(|| "active".into()),
        credential_ref: req
            .credential_ref
            .clone()
            .or_else(|| existing.as_ref().and_then(|a| a.credential_ref.clone())),
        extra,
    };
    storage
        .upsert_ad_account(&account)
        .await
        .map_err(|e| e.to_string())?;

    let mut replaced_bindings = 0usize;
    if req.replace_existing_bindings {
        replaced_bindings = storage
            .delete_person_account_bindings_for_account(&ad_account_id)
            .await
            .map_err(|e| e.to_string())?;
    }
    let mut created_bindings = Vec::with_capacity(req.person_bindings.len());
    for b in &req.person_bindings {
        let binding = PersonAccountBinding {
            id: format!("bind-{}-{}", ad_account_id, &Uuid::new_v4().simple().to_string()[..8]),
            person_id: b.person_id.clone(),
            ad_account_id: ad_account_id.clone(),
            relation_role: b.relation_role.clone(),
            split_weight: b.split_weight,
            effective_from: b.effective_from,
            effective_to: b.effective_to,
        };
        storage
            .upsert_person_account_binding(&binding)
            .await
            .map_err(|e| e.to_string())?;
        created_bindings.push(binding);
    }

    let mut replaced_project_links = 0usize;
    if req.replace_existing_project_links {
        replaced_project_links = storage
            .delete_project_account_links_for_account(&ad_account_id)
            .await
            .map_err(|e| e.to_string())?;
    }
    let mut created_links = Vec::with_capacity(req.project_links.len());
    for l in &req.project_links {
        let link = ProjectAccountLink {
            id: format!("pal-{}-{}", l.project_id, &Uuid::new_v4().simple().to_string()[..8]),
            project_id: l.project_id.clone(),
            ad_account_id: ad_account_id.clone(),
            campaign_filter_regex: l.campaign_filter_regex.clone(),
            allocation_weight: l.allocation_weight,
        };
        storage
            .upsert_project_account_link(&link)
            .await
            .map_err(|e| e.to_string())?;
        created_links.push(link);
    }

    let _ = storage
        .append_audit_log(&AuditLogEntry {
            id: format!("aud-{}", Uuid::new_v4()),
            timestamp: Utc::now(),
            operator: source.to_string(),
            source: source.to_string(),
            platform: req.platform,
            ad_account_id: ad_account_id.clone(),
            action: format!("relabel_governance({:?})", req.auth_strategy),
            entity_ids: vec![req.platform_account_id],
            dry_run: false,
            status: "executed".into(),
            reason: "重标人-户-项目关系".into(),
            detail: json!({
                "replaced_bindings": replaced_bindings,
                "new_bindings_count": created_bindings.len(),
                "replaced_project_links": replaced_project_links,
                "new_project_links_count": created_links.len()
            }),
        })
        .await;

    Ok(json!({
        "status": "relabeled",
        "auth_strategy": req.auth_strategy,
        "account": account,
        "replaced_bindings": replaced_bindings,
        "person_bindings": created_bindings,
        "replaced_project_links": replaced_project_links,
        "project_links": created_links
    }))
}

/// Fallback Tier-2 workflow: creates/updates a custom project and binds discovered sub-accounts
/// plus optional owner `人-户关系` bindings in one step.
pub async fn fallback_bootstrap_project(
    kernel: &Kernel,
    source: &str,
    req: FallbackBootstrapProjectRequest,
) -> Result<Value, String> {
    let storage = kernel
        .require_service::<dyn StorageService>()
        .map_err(|e| e.to_string())?;

    storage
        .upsert_project(&req.project)
        .await
        .map_err(|e| e.to_string())?;

    let mut accounts_out = Vec::with_capacity(req.accounts.len());
    let mut links_out = Vec::with_capacity(req.accounts.len());
    let mut bindings_out = Vec::new();
    let default_date = req.project.start_date;

    for item in &req.accounts {
        let ad_account_id = format!("acc-{}-{}", item.platform.as_str(), item.platform_account_id);
        let account = AdAccount {
            id: ad_account_id.clone(),
            org_id: req.project.org_id.clone(),
            platform: item.platform,
            platform_account_id: item.platform_account_id.clone(),
            account_name: item.account_name.clone(),
            account_type: item.account_type.clone().unwrap_or(AccountType::AgencySub),
            rebate_rate: item.rebate_rate.unwrap_or(0.05),
            service_fee_rate: item.service_fee_rate.unwrap_or(0.08),
            default_cash_ratio: item.default_cash_ratio.unwrap_or(0.9),
            status: "active".into(),
            credential_ref: None,
            extra: json!({
                "auth_strategy": AuthLoginStrategy::DirectApiAccountDiscovery,
                "bootstrapped_project_id": req.project.id
            }),
        };
        storage
            .upsert_ad_account(&account)
            .await
            .map_err(|e| e.to_string())?;

        if req.replace_existing_links {
            let _ = storage
                .delete_project_account_links_for_account(&ad_account_id)
                .await;
        }
        let link = ProjectAccountLink {
            id: format!("pal-{}-{}", req.project.id, &ad_account_id),
            project_id: req.project.id.clone(),
            ad_account_id: ad_account_id.clone(),
            campaign_filter_regex: item.campaign_filter_regex.clone(),
            allocation_weight: item.allocation_weight.unwrap_or(1.0),
        };
        storage
            .upsert_project_account_link(&link)
            .await
            .map_err(|e| e.to_string())?;
        links_out.push(link);

        if let Some(person_id) = &item.owner_person_id {
            let binding = PersonAccountBinding {
                id: format!("bind-{}-{}-{}", req.project.id, ad_account_id, person_id),
                person_id: person_id.clone(),
                ad_account_id: ad_account_id.clone(),
                relation_role: item
                    .relation_role
                    .clone()
                    .unwrap_or(BindingRole::PrimaryOptimizer),
                split_weight: item.split_weight.unwrap_or(1.0),
                effective_from: item.effective_from.unwrap_or(default_date),
                effective_to: None,
            };
            storage
                .upsert_person_account_binding(&binding)
                .await
                .map_err(|e| e.to_string())?;
            bindings_out.push(binding);
        }

        let _ = storage
            .append_audit_log(&AuditLogEntry {
                id: format!("aud-{}", Uuid::new_v4()),
                timestamp: Utc::now(),
                operator: source.to_string(),
                source: source.to_string(),
                platform: item.platform,
                ad_account_id,
                action: "fallback_bootstrap_project".into(),
                entity_ids: vec![req.project.id.clone(), item.platform_account_id.clone()],
                dry_run: false,
                status: "executed".into(),
                reason: "降级拉取子账户并自建项目关联".into(),
                detail: json!({ "project_id": req.project.id }),
            })
            .await;

        accounts_out.push(account);
    }

    Ok(json!({
        "status": "bootstrapped",
        "auth_strategy": AuthLoginStrategy::DirectApiAccountDiscovery,
        "project": req.project,
        "linked_accounts": accounts_out,
        "project_links": links_out,
        "person_bindings": bindings_out
    }))
}
