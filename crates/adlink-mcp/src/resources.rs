//! MCP Resources exposing live platform catalogs, caliber definitions, onboarding manuals, and governance summaries.

use std::sync::Arc;

use adlink_domain::PlatformId;
use adlink_engine::AdPlatformConnector;
use adlink_kernel::Kernel;
use adlink_storage::StorageService;
use serde_json::{json, Value};

pub fn list_resources() -> Value {
    json!({
        "resources": [
            {
                "uri": "adlink://catalog/platforms",
                "name": "Supported Ad Platforms & Official Field Mappings",
                "description": "Catalog of all 15 domestic CN (10) & global overseas (5) ad platforms, default currencies, chunk windows, QPS limits, and official field mappings",
                "mimeType": "application/json"
            },
            {
                "uri": "adlink://calibers/definitions",
                "name": "Metric Caliber Definitions (消耗口径/业绩口径/自定义口径)",
                "description": "Standard Spend/Performance calibers plus all custom AST formula calibers registered in AdLink",
                "mimeType": "application/json"
            },
            {
                "uri": "adlink://guide/onboarding",
                "name": "Agent Official API Onboarding Playbook",
                "description": "Step-by-step instructions for AI agents to guide users through connecting official platform credentials",
                "mimeType": "application/json"
            },
            {
                "uri": "adlink://governance/summary",
                "name": "Organization, People, Accounts & Projects Summary",
                "description": "Live snapshot of 人-户关系 bindings, project classifications, and creative asset counts",
                "mimeType": "application/json"
            }
        ]
    })
}

pub async fn read_resource(kernel: &Arc<Kernel>, uri: &str) -> Result<Value, String> {
    let text = match uri {
        "adlink://catalog/platforms" => {
            let metas: Vec<_> = PlatformId::ALL.iter().map(|p| p.meta()).collect();
            serde_json::to_string_pretty(&metas).map_err(|e| e.to_string())?
        }
        "adlink://calibers/definitions" => {
            let storage = kernel
                .require_service::<dyn StorageService>()
                .map_err(|e| e.to_string())?;
            let custom = storage
                .list_custom_calibers()
                .await
                .map_err(|e| e.to_string())?;
            let payload = json!({
                "spend_calibers": ["gross_media_cost", "cash_cost", "net_after_rebate_cost", "full_funnel_cost"],
                "performance_calibers": ["agency_gross_billing", "agency_net_revenue", "platform_gmv", "brand_net_roi"],
                "custom_calibers": custom
            });
            serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?
        }
        "adlink://guide/onboarding" => {
            let mut guides = Vec::new();
            for &p in PlatformId::ALL {
                if let Some(conn) = kernel.get_named_service::<dyn AdPlatformConnector>(p.as_str()) {
                    guides.push(conn.onboard_guide());
                }
            }
            serde_json::to_string_pretty(&guides).map_err(|e| e.to_string())?
        }
        "adlink://governance/summary" => {
            let storage = kernel
                .require_service::<dyn StorageService>()
                .map_err(|e| e.to_string())?;
            let orgs = storage.list_organizations().await.map_err(|e| e.to_string())?;
            let people = storage.list_people().await.map_err(|e| e.to_string())?;
            let accounts = storage
                .list_ad_accounts(None)
                .await
                .map_err(|e| e.to_string())?;
            let bindings = storage
                .list_person_account_bindings()
                .await
                .map_err(|e| e.to_string())?;
            let projects = storage.list_projects().await.map_err(|e| e.to_string())?;
            let creatives = storage
                .list_creative_assets()
                .await
                .map_err(|e| e.to_string())?;
            let payload = json!({
                "organizations": orgs,
                "people_count": people.len(),
                "accounts": accounts,
                "person_account_bindings_count": bindings.len(),
                "projects": projects,
                "creative_assets_count": creatives.len(),
            });
            serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?
        }
        other => return Err(format!("unknown resource URI '{other}'")),
    };

    Ok(json!({
        "contents": [
            {
                "uri": uri,
                "mimeType": "application/json",
                "text": text
            }
        ]
    }))
}
