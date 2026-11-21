//! MCP Prompts helping AI agents onboard official platform APIs and run executive ad reviews.

use serde_json::{json, Value};

pub fn list_prompts() -> Value {
    json!({
        "prompts": [
            {
                "name": "onboard_ad_platform",
                "description": "Guide the user step-by-step to obtain and verify official Marketing API credentials for a Chinese ad platform",
                "arguments": [
                    {
                        "name": "platform",
                        "description": "Platform ID (e.g. tencent_ads, xhs_juguang, xhs_chengfeng, xhs_pugongying, bilibili_sanlian, bilibili_huahuo, kuaishou, ocean_engine_ad, ocean_engine_local, alipay_denghuo)",
                        "required": true
                    }
                ]
            },
            {
                "name": "daily_agency_briefing",
                "description": "Generate an executive daily briefing reconciling Gross Media Cost, Cash Cost, Net Revenue, and Project ROI",
                "arguments": [
                    {
                        "name": "start_date",
                        "description": "YYYY-MM-DD",
                        "required": true
                    },
                    {
                        "name": "end_date",
                        "description": "YYYY-MM-DD",
                        "required": true
                    }
                ]
            },
            {
                "name": "creative_workload_review",
                "description": "Analyze creative formats, opening hooks, internal creator labor hours, and KOL seeding+boost attribution",
                "arguments": [
                    {
                        "name": "start_date",
                        "description": "YYYY-MM-DD",
                        "required": true
                    },
                    {
                        "name": "end_date",
                        "description": "YYYY-MM-DD",
                        "required": true
                    }
                ]
            }
        ]
    })
}

pub fn get_prompt(name: &str, args: Option<&Value>) -> Result<Value, String> {
    let arg_str = |key: &str, default: &str| -> String {
        args.and_then(|a| a.get(key))
            .and_then(Value::as_str)
            .unwrap_or(default)
            .to_string()
    };

    let messages = match name {
        "onboard_ad_platform" => {
            let platform = arg_str("platform", "tencent_ads");
            vec![json!({
                "role": "user",
                "content": {
                    "type": "text",
                    "text": format!(
                        "请帮我把广告平台 `{platform}` 接入到 AdLink。\n\
                        步骤要求：\n\
                        1. 先调用 `adlink_onboard_guide`（platform=`{platform}`）与 `adlink_creation_status_and_risks`，说明官方开放平台认证方式与注意事项；\n\
                        2. 【双层登录策略】\n\
                           - 最优方案：若该媒体平台已有官方 MCP Server，请优先引导我通过官方 MCP 完成登录鉴权并配置 `official_mcp_endpoint`，随后在 AdLink 调用 `adlink_discover_platform_accounts` 和 `adlink_relabel_account_governance` 重标内部「人-户关系」与「项目归属」；\n\
                           - 降级方案：若无官方 MCP，则指导我配置 API 凭据，先调用 `adlink_discover_platform_accounts` 拉取账户列表，再调用 `adlink_fallback_bootstrap_project` 自建项目并关联子账户；\n\
                        3. 如果是蒲公英或花火这类没有公开自助 API 的平台，向我说明合规白名单接口与 `adlink_ingest_external_records` 导表打通方案；\n\
                        4. 【强制安全询问】默认仅引导接入只读拉数权限。你必须主动询问我：『是否需要接入广告创编（新建计划/单元/创意）能力？』，并完整提示创编的 5 项核心风险（误烧预算及单位差异、审核拒审与封户、定向/归因错配、平台接口差异、默认 PAUSED+dry_run 护栏）；未得我确认前，严禁开启 `[ad_creation]`；\n\
                        5. 指导我完成后调用 `adlink_health_check` 和 `adlink_sync_reports` 验证拉数结果。"
                    )
                }
            })]
        }
        "daily_agency_briefing" => {
            let sd = arg_str("start_date", "2026-11-01");
            let ed = arg_str("end_date", "2026-11-21");
            vec![json!({
                "role": "user",
                "content": {
                    "type": "text",
                    "text": format!(
                        "请生成 {sd} 至 {ed} 的代理商/品牌投放经营日报：\n\
                        1. 调用 `adlink_reconcile_calibers` 输出账面消耗、现金消耗、赠款消耗、返点后净消耗、代理商净营收、GMV 与自定义口径对账；\n\
                        2. 分别调用 `adlink_query_metrics` 按 `platform`、`project_type`、`person` 分组查看各媒体、各项目类型与各优化师归因业绩；\n\
                        3. 给出高消耗异常/ROI 波动提示（如需调整计划先用 `dry_run: true`）。"
                    )
                }
            })]
        }
        "creative_workload_review" => {
            let sd = arg_str("start_date", "2026-11-01");
            let ed = arg_str("end_date", "2026-11-21");
            vec![json!({
                "role": "user",
                "content": {
                    "type": "text",
                    "text": format!(
                        "请调用 `adlink_creative_attribution_report` 分析 {sd} 至 {ed} 的创意表现与内部创作工作量产投比：\n\
                        1. 对比不同创意格式与钩子类型（痛点/对比/开箱/剧情/权威）的 ROI 与 CTR；\n\
                        2. 汇报编导/拍摄/剪辑/设计每位创作者的工时投入与 `gmv_per_work_hour_yuan`；\n\
                        3. 重点分析小红书蒲公英+聚光/乘风（note_id）与 B站花火+三连起飞（bvid）的种草加热全链路 ROI。"
                    )
                }
            })]
        }
        other => return Err(format!("unknown prompt '{other}'")),
    };

    Ok(json!({
        "description": name,
        "messages": messages
    }))
}
