//! Schema-Driven UI contract (`/api/v1/ui/schema`) allowing user-built Web portals,
//! WeChat MiniPrograms, UniApp, or Taro clients to render views dynamically.

use adlink_domain::PlatformId;
use serde_json::{json, Value};

pub fn ui_schema() -> Value {
    let platform_options: Vec<Value> = PlatformId::ALL
        .iter()
        .map(|p| {
            json!({
                "value": p.as_str(),
                "label_zh": p.display_name_zh(),
                "label_en": p.display_name_en(),
                "access_mode": p.meta().access_mode
            })
        })
        .collect();

    json!({
        "schema_version": "1.0",
        "product_name": "AdLink 智投数据底座",
        "navigation": [
            { "id": "overview", "title_zh": "口径对账总览", "title_en": "Caliber Overview", "endpoint": "/api/v1/analytics/reconcile" },
            { "id": "governance", "title_zh": "人户关系与项目分类", "title_en": "Governance", "endpoint": "/api/v1/bindings" },
            { "id": "creatives", "title_zh": "创意归因与创作工时", "title_en": "Creative Attribution", "endpoint": "/api/v1/attribution/creatives" },
            { "id": "sync", "title_zh": "高并发同步与接入指引", "title_en": "Sync & Onboarding", "endpoint": "/api/v1/platforms" },
            { "id": "agents", "title_zh": "多端 Agent MCP 接入", "title_en": "Agent MCP Config", "endpoint": "/api/v1/plugins" }
        ],
        "caliber_selectors": {
            "spend": [
                { "id": "gross_media_cost_yuan", "label": "账面总消耗 (现金+赠款)" },
                { "id": "cash_cost_yuan", "label": "现金消耗" },
                { "id": "net_after_rebate_cost_yuan", "label": "返点后净消耗" },
                { "id": "full_funnel_cost_yuan", "label": "全域投入成本 (含商单+制作)" }
            ],
            "performance": [
                { "id": "platform_gmv_yuan", "label": "平台回传 GMV" },
                { "id": "agency_gross_billing_yuan", "label": "代理商毛流水" },
                { "id": "agency_net_revenue_yuan", "label": "代理商净营收 (返点+服务费)" }
            ]
        },
        "platforms": platform_options,
        "supported_clients": [
            { "id": "workbuddy", "name": "WorkBuddy", "transport": "stdio / streamable-http" },
            { "id": "kimiwork", "name": "KimiWork", "transport": "streamable-http / sse" },
            { "id": "traework", "name": "TraeWork", "transport": "stdio / streamable-http" },
            { "id": "doubao", "name": "豆包工作", "transport": "streamable-http / sse" },
            { "id": "qianwen", "name": "千问办公", "transport": "streamable-http / sse" },
            { "id": "claude-code", "name": "Claude Code", "transport": "stdio / http" },
            { "id": "codex", "name": "OpenAI Codex", "transport": "stdio / http" },
            { "id": "antigravity", "name": "Antigravity", "transport": "stdio / http" },
            { "id": "cursor", "name": "Cursor", "transport": "stdio / streamable-http" },
            { "id": "grokbot", "name": "GrokBot", "transport": "streamable-http" },
            { "id": "muse", "name": "Muse", "transport": "streamable-http / stdio" },
            { "id": "dots", "name": "Dots", "transport": "streamable-http / stdio" }
        ]
    })
}
