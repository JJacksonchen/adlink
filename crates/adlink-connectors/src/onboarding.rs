//! Structured API onboarding walkthroughs designed to guide both human engineers
//! and AI coding/office agents (WorkBuddy, KimiWork, Claude Code, Cursor, Antigravity, Codex)
//! through connecting official ad platform APIs step by step.

use adlink_domain::{AdCreationRiskDisclosure, PlatformId};
use adlink_engine::{CredentialFieldSpec, PlatformOnboardGuide};

/// Returns the complete onboarding guide for `platform` in `mode` (`mock` or `live`).
pub fn build_onboard_guide(platform: PlatformId, mode: &str) -> PlatformOnboardGuide {
    let meta = platform.meta();
    let optimal_login_strategy = "【最优登录方案】若媒体平台已提供官方 MCP Server，建议用户先通过官方 MCP 登录鉴权接口完成授权，配置 `auth_strategy = \"official_mcp_bridge\"` 与 `official_mcp_endpoint`，随后在 AdLink 调用 `adlink_discover_platform_accounts` 拉取已鉴权账户，再调用 `adlink_relabel_account_governance` 在 AdLink 内自由重标企业内部「人-户关系」与「项目归属」，无需重复开发 OAuth 页面。".into();
    let fallback_login_strategy = "【降级登录方案】若官方 MCP 暂不可用或需服务器无头对接，可配置开放平台应用凭据并调用 `adlink_discover_platform_accounts` 直接拉取媒体账户/子账户列表，随后调用 `adlink_fallback_bootstrap_project` 自行创建企业项目并一键关联选中子账户与负责优化师。".into();
    let ad_creation_ask_prompt = "【创编接入默认询问（默认关闭）】在协助用户完成认证对接时，务必主动询问用户：『是否需要接入广告创编（新建计划/单元/创意）能力？』并明确告知创编风险；未经用户确认并显式配置 `[ad_creation] enabled = true, risk_acknowledged = true` 前，严禁开启创编能力。".into();
    let ad_creation_risks = AdCreationRiskDisclosure::standard_risk_warnings();

    match platform {
        PlatformId::TencentAds => PlatformOnboardGuide {
            platform,
            mode: mode.into(),
            developer_portal_url: "https://developers.e.qq.com/".into(),
            oauth_or_auth_flow: "OAuth 2.0 Authorization Code (access_token + refresh_token) / Official MCP Bridge".into(),
            optimal_login_strategy,
            fallback_login_strategy,
            ad_creation_support: meta.ad_creation_support,
            ad_creation_ask_prompt,
            ad_creation_risks,
            required_permissions: vec![
                "ads_insights (日报表/小时报/异步任务，注意必须传 group_by)".into(),
                "account_management (advertiser/get 账户发现 & funds/get 资金账户)".into(),
                "ads_management (可选：仅在用户确认开启运维/创编后申请 adgroups/add, dynamic_creatives/add)".into(),
            ],
            prerequisite_steps: vec![
                "1. 优先确认是否已有腾讯广告官方 MCP 登录鉴权通道；若有，配置 official_mcp_endpoint 并调用 adlink_relabel_account_governance 重标人-户-项目关系。".into(),
                "2. 若走 API 降级直连：在腾讯广告开发者平台创建应用并完成 OAuth2 授权获取 access_token 与 refresh_token（建议每 20 小时刷新）。".into(),
                "3. 在 config/adlink.toml 的 [connectors.tencent_ads] 配置 live 模式凭据（支持填写 official_mcp_endpoint），并保持 [ad_creation].enabled = false 直至用户明确确认开启创编。".into(),
            ],
            credential_fields: vec![
                spec("app_id", true, false, "腾讯广告应用 client_id", "1109283746"),
                spec("app_secret", true, true, "应用 client_secret", "env:TENCENT_ADS_SECRET"),
                spec("access_token", true, true, "OAuth2 access_token", "env:TENCENT_ADS_TOKEN"),
                spec("refresh_token", false, true, "OAuth2 refresh_token", "env:TENCENT_ADS_REFRESH"),
                spec("official_mcp_endpoint", false, false, "可选：官方 MCP 鉴权服务地址 (Tier 1 最优方案)", "https://mcp.example.com/tencent-ads"),
            ],
            toml_snippet: r#"[connectors.tencent_ads]
mode = "live"
auth_strategy = "official_mcp_bridge" # 无官方 MCP 时改为 "direct_api_account_discovery"
app_id = "1109283746"
app_secret = "${TENCENT_ADS_SECRET}"
access_token = "${TENCENT_ADS_TOKEN}""#.into(),
            verification_steps: vec![
                "运行 `adlink doctor` 检查连接器健康与 `[ad_creation]` 默认关闭状态。".into(),
                "调用 `adlink_discover_platform_accounts` 拉取账户列表并重标或创建项目关联子账户。".into(),
                "调用 `adlink_sync_reports` 验证 v3.0 指标 (`cost`, `view_count`, `valid_click_count`, `cheout_fd`, `cheout_pv_1d`) 准确入库。".into(),
            ],
            gotchas: vec![
                meta.compliance_note.into(),
                "AMS v3.0 为「广告组 + 动态创意」两层结构，且 daily_reports/get 不传 group_by 会仅返回账户日汇总；AdLink 已自动匹配各层级 group_by。".into(),
            ],
        },
        PlatformId::XhsJuguang | PlatformId::XhsChengfeng => PlatformOnboardGuide {
            platform,
            mode: mode.into(),
            developer_portal_url: "https://open.xiaohongshu.com/".into(),
            oauth_or_auth_flow: "OAuth 2.0 (Header: Access-Token) / Official MCP Bridge".into(),
            optimal_login_strategy,
            fallback_login_strategy,
            ad_creation_support: meta.ad_creation_support,
            ad_creation_ask_prompt,
            ad_creation_risks,
            required_permissions: vec![
                "聚光/乘风报表查询 (/api/open/jg/data/report/* & /api/open/cf/data/report/*)".into(),
                "账户列表与余额查询 (/api/open/jg/account/list, /api/open/jg/account/balance/info)".into(),
                "可选写权限：仅用户确认风险后申请 campaign/create, unit/create, creativity/create".into(),
            ],
            prerequisite_steps: vec![
                "1. 联系小红书商业化对接商务或代理商管理员开通聚光/乘风开放平台白名单资质（或配置官方 MCP 鉴权桥接）。".into(),
                "2. 完成 OAuth2 授权获取 Access-Token，并调用 adlink_discover_platform_accounts 拉取子账户列表进行人-户/项目关联。".into(),
                "3. 若白名单文档约定报表 `fee` 为元字符串，可在 connector extra 中设置 `report_money_unit = \"yuan\"`（AdLink 亦会自动识别带小数点金额）。".into(),
            ],
            credential_fields: vec![
                spec("app_id", true, false, "小红书开放平台 App ID", "xhs_app_001"),
                spec("app_secret", true, true, "小红书开放平台 App Secret", "env:XHS_APP_SECRET"),
                spec("access_token", true, true, "Header Access-Token 令牌", "env:XHS_ACCESS_TOKEN"),
                spec("official_mcp_endpoint", false, false, "可选：官方 MCP 鉴权服务地址", "https://mcp.example.com/xhs"),
            ],
            toml_snippet: format!(
                "[connectors.{}]\nmode = \"live\"\napp_id = \"xhs_app_001\"\napp_secret = \"${{XHS_APP_SECRET}}\"\naccess_token = \"${{XHS_ACCESS_TOKEN}}\"",
                platform.as_str()
            ),
            verification_steps: vec![
                "调用 `/api/open/jg/account/balance/info` 校验现金余额与今日消耗（单位：分）。".into(),
                "确保报表保留 `note_id` 字段以打通蒲公英种草+聚光乘风加热归因。".into(),
            ],
            gotchas: vec![
                meta.compliance_note.into(),
                "聚光与乘风共用底层资金账户；完整离线报表/创编字段明细以小红书白名单《聚光开放平台推广白皮书》为准。".into(),
            ],
        },
        PlatformId::XhsPugongying | PlatformId::BilibiliHuahuo => PlatformOnboardGuide {
            platform,
            mode: mode.into(),
            developer_portal_url: meta.docs_url.into(),
            oauth_or_auth_flow: "官方认证服务商白名单 API 或 结构化 CSV/JSON 合规导入 (`POST /api/v1/ingest` / MCP `adlink_ingest_external_records`)".into(),
            optimal_login_strategy,
            fallback_login_strategy,
            ad_creation_support: meta.ad_creation_support,
            ad_creation_ask_prompt,
            ad_creation_risks,
            required_permissions: vec![
                "品牌方/MCN认证机构商务特许接口权限（若已获官方白名单）".into(),
                "或官方后台导出的订单结算明细（含 note_id / bvid 与订单金额）".into(),
            ],
            prerequisite_steps: vec![
                "1. 注意：小红书蒲公英与B站花火无面向普通公众的免审批自助公开 API（亦无自助创编 API），AdLink 严格遵守合规原则，不提供任何逆向抓包脚本。".into(),
                "2. 若您的企业已获得官方服务商/品牌白名单接口权限，请在 config 中填写 partner token 与 `base_url_override`。".into(),
                "3. 若未开通白名单 API，请将官方后台导出的商单表通过 `adlink_ingest_external_records` MCP 工具或 `POST /api/v1/ingest` 导入，并通过 `adlink_relabel_account_governance` 绑定项目后自动通过 `note_id` / `bvid` 打通全链路归因！".into(),
            ],
            credential_fields: vec![
                spec("access_token", false, true, "特许合作方 Token（仅白名单机构需填写）", "env:PARTNER_TOKEN"),
                spec("base_url_override", false, false, "白名单专用网关 URL", "https://partner-gateway.example.com"),
            ],
            toml_snippet: format!(
                "[connectors.{}]\nmode = \"mock\" # 获白名单后改为 \"live\" 并配置 base_url_override；或直接使用 /api/v1/ingest 导入官方导出文件",
                platform.as_str()
            ),
            verification_steps: vec![
                "导入或同步商单后，调用 `adlink_creative_attribution_report` 查看 `seeding_boost_attribution` 中的 `note_id` / `bvid` 联合 ROI。".into(),
            ],
            gotchas: vec![
                meta.compliance_note.into(),
                "导入时务必带上 `note_id`（小红书）或 `bvid`（B站）以激活跨平台归因。".into(),
            ],
        },
        PlatformId::BilibiliSanlian => PlatformOnboardGuide {
            platform,
            mode: mode.into(),
            developer_portal_url: "https://openhome.bilibili.com/".into(),
            oauth_or_auth_flow: "Bearer Token + X-Call-Source Header (cm.bilibili.com/takumi/api) / Official MCP Bridge".into(),
            optimal_login_strategy,
            fallback_login_strategy,
            ad_creation_support: meta.ad_creation_support,
            ad_creation_ask_prompt,
            ad_creation_risks,
            required_permissions: vec![
                "B站商业化合作方三连/起飞报表与账户余额读取权限".into(),
                "可选写权限：仅用户确认开启创编后申请合作方计划/单元/创意写接口".into(),
            ],
            prerequisite_steps: vec![
                "1. 向 B 站商业化对接经理申请代理商/大客户 MAPI 白名单，获取 client credentials 与 `X-Call-Source`（如需 `X-Sanlian-Skills-Version` 可配置在 `extra` 中）。".into(),
                "2. 获取 OAuth2 access_token 或官方 MCP 鉴权桥接地址，配置到 [connectors.bilibili_sanlian]。".into(),
            ],
            credential_fields: vec![
                spec("access_token", true, true, "Bearer access_token", "env:BILI_ACCESS_TOKEN"),
                spec("call_source", true, false, "B站分配的 X-Call-Source 标识", "adlink-agency-gateway"),
                spec("official_mcp_endpoint", false, false, "可选：官方 MCP 鉴权服务地址", "https://mcp.example.com/bilibili"),
            ],
            toml_snippet: r#"[connectors.bilibili_sanlian]
mode = "live"
access_token = "${BILI_ACCESS_TOKEN}"
call_source = "adlink-agency-gateway""#.into(),
            verification_steps: vec![
                "使用 `adlink_sync_reports` 拉取三连起飞报表，验证 `cost`(分)、`play_cnt`、`coin_cnt` 与 `bvid` 正确入库。".into(),
            ],
            gotchas: vec![meta.compliance_note.into()],
        },
        PlatformId::Kuaishou => PlatformOnboardGuide {
            platform,
            mode: mode.into(),
            developer_portal_url: "https://developers.e.kuaishou.com/".into(),
            oauth_or_auth_flow: "OAuth 2.0 (Header: Access-Token) / Official MCP Bridge".into(),
            optimal_login_strategy,
            fallback_login_strategy,
            ad_creation_support: meta.ad_creation_support,
            ad_creation_ask_prompt,
            ad_creation_risks,
            required_permissions: vec![
                "数据报表 (/rest/openapi/v1/report/*) 与代理商账户发现 (/v1/agent/advertiser/list)".into(),
                "可选写权限：仅用户确认风险后申请 campaign/create, v2/ad_unit/create, v2/creative/create".into(),
            ],
            prerequisite_steps: vec![
                "1. 在快手磁力引擎开放平台完成企业开发者认证与 App 创建（或复用官方 MCP 鉴权）。".into(),
                "2. 通过 OAuth2 获取广告主授权 access_token 与 refresh_token，使用 `adlink_discover_platform_accounts` 拉取账户并完成人-户/项目重标或创建。".into(),
            ],
            credential_fields: vec![
                spec("app_id", true, false, "快手开放平台 app_id", "192039"),
                spec("app_secret", true, true, "快手开放平台 secret", "env:KS_APP_SECRET"),
                spec("access_token", true, true, "Access-Token", "env:KS_ACCESS_TOKEN"),
                spec("official_mcp_endpoint", false, false, "可选：官方 MCP 鉴权服务地址", "https://mcp.example.com/kuaishou"),
            ],
            toml_snippet: r#"[connectors.kuaishou]
mode = "live"
app_id = "192039"
app_secret = "${KS_APP_SECRET}"
access_token = "${KS_ACCESS_TOKEN}""#.into(),
            verification_steps: vec![
                "校验 `charge`（元）与 `form_count`/`submit`/`event_pay`/`activation` 准确映射，且无 `has_more` 字段时通过 `total_count` 自动翻页。".into(),
            ],
            gotchas: vec![
                meta.compliance_note.into(),
                "实时报表单次跨度上限 7 天；注意报表 `charge` 单位为「元」，而创编接口预算/出价单位为「厘」(1元=1000厘)，AdLink 会在内部严格换算。".into(),
            ],
        },
        PlatformId::OceanEngineAd | PlatformId::OceanEngineLocal => PlatformOnboardGuide {
            platform,
            mode: mode.into(),
            developer_portal_url: "https://open.oceanengine.com/".into(),
            oauth_or_auth_flow: "OAuth 2.0 (Header: Access-Token) / Official MCP Bridge".into(),
            optimal_login_strategy,
            fallback_login_strategy,
            ad_creation_support: meta.ad_creation_support,
            ad_creation_ask_prompt,
            ad_creation_risks,
            required_permissions: vec![
                "ocean_engine.reports.query (/open_api/v3.0/report/custom/get/ & local/report/*)".into(),
                "已授权账户拉取 (/open_api/oauth2/advertiser/get/) 与账户日流水".into(),
                "可选写权限：仅用户确认风险后申请 project/create & promotion/create".into(),
            ],
            prerequisite_steps: vec![
                "1. 登录巨量引擎开放平台创建应用（或复用官方 MCP 登录鉴权并配置 official_mcp_endpoint），勾选 v3.0 报表、本地推与账户查询权限。".into(),
                "2. 获取 access_token（24h 有效，refresh_token 30d），通过 `adlink_discover_platform_accounts` 发现所有已授权广告主/本地推子账户并绑定人-户/项目关系。".into(),
            ],
            credential_fields: vec![
                spec("app_id", true, false, "巨量开放平台 App ID", "178293840192"),
                spec("app_secret", true, true, "App Secret", "env:OCEAN_APP_SECRET"),
                spec("access_token", true, true, "OAuth2 Access-Token", "env:OCEAN_ACCESS_TOKEN"),
                spec("official_mcp_endpoint", false, false, "可选：官方 MCP 鉴权服务地址", "https://mcp.example.com/ocean"),
            ],
            toml_snippet: format!(
                "[connectors.{}]\nmode = \"live\"\napp_id = \"178293840192\"\napp_secret = \"${{OCEAN_APP_SECRET}}\"\naccess_token = \"${{OCEAN_ACCESS_TOKEN}}\"",
                platform.as_str()
            ),
            verification_steps: vec![
                "拉取 `report/custom/get/`（创意层自动切换 `data_topic=MATERIAL_DATA`）验证 `stat_cost`(元→分)、`show_cnt`、`click_cnt`、`convert_cnt` 入库。".into(),
            ],
            gotchas: vec![
                meta.compliance_note.into(),
                "巨量报告与日流水费用单位均为元（`stat_cost` / `pay_order_amount`），AdLink 边界统一转分存储。".into(),
            ],
        },
        PlatformId::AlipayDenghuo => PlatformOnboardGuide {
            platform,
            mode: mode.into(),
            developer_portal_url: "https://open.alipay.com/".into(),
            oauth_or_auth_flow: "Alipay OpenAPI RSA2 (SHA256WithRSA) + biz_token + alipay_pid".into(),
            optimal_login_strategy,
            fallback_login_strategy,
            ad_creation_support: meta.ad_creation_support,
            ad_creation_ask_prompt,
            ad_creation_risks,
            required_permissions: vec![
                "支付宝灯火广告投放数据查询 (alipay.data.dataservice.ad.reportdata.query)".into(),
                "灯火消费明细查询 (alipay.data.dataservice.ad.consumehistory.query)".into(),
                "转化数据回传与推广落地页 (conversion.upload / landinginfo.createormodify)".into(),
            ],
            prerequisite_steps: vec![
                "1. 登录支付宝开放平台创建应用并绑定“支付宝广告投放（灯火）”能力包。".into(),
                "2. 生成 PKCS#8 RSA2 2048位密钥对，在开放平台上传应用公钥，将私钥保存至安全文件路径或环境变量。".into(),
                "3. 从灯火代理商/商家后台获取 `biz_token` 与 `2088` 开头的 18 位 `alipay_pid`；若需拉取特定订单 GMV 转化指标，请在 connector `extra.conv_code_list` 配置转化事件 Code。".into(),
            ],
            credential_fields: vec![
                spec("app_id", true, false, "支付宝应用 app_id", "2021004192837465"),
                spec("rsa2_private_key_pem", true, true, "PKCS#8 RSA2 私钥内容或PEM", "env:ALIPAY_RSA2_PRIVATE_KEY"),
                spec("biz_token", true, true, "灯火平台权限 biz_token", "env:ALIPAY_BIZ_TOKEN"),
                spec("alipay_pid", true, false, "2088开头18位商家/代理商PID", "2088612093847102"),
            ],
            toml_snippet: r#"[connectors.alipay_denghuo]
mode = "live"
app_id = "2021004192837465"
rsa2_private_key_pem = "${ALIPAY_RSA2_PRIVATE_KEY}"
biz_token = "${ALIPAY_BIZ_TOKEN}"
alipay_pid = "2088612093847102""#.into(),
            verification_steps: vec![
                "AdLink 内置纯 Rust RSA2 签名器 (`ring::signature::RsaKeyPair`)，无需安装 Java/PHP SDK。".into(),
                "执行 `adlink sync --platforms alipay_denghuo` 验证 `cost`(分)、`impression`、`click` 与 `conversion_data_list` 解析。".into(),
            ],
            gotchas: vec![
                meta.compliance_note.into(),
                "官方 `reportdata.query` 明细数组字段为 `conversion_data_list`（`PRINCIPAL` / `MARKET_TARGET` 层级不返回该字段）；支付宝灯火开放平台目前未公开自助计划/单元/创意创编及状态修改 OpenAPI。".into(),
            ],
        },
        PlatformId::TikTokAds => PlatformOnboardGuide {
            platform,
            mode: mode.into(),
            developer_portal_url: "https://business-api.tiktok.com/portal/docs".into(),
            oauth_or_auth_flow: "TikTok Marketing API v1.3 OAuth 2.0 (Access-Token header) / Official MCP Bridge".into(),
            optimal_login_strategy,
            fallback_login_strategy,
            ad_creation_support: meta.ad_creation_support,
            ad_creation_ask_prompt,
            ad_creation_risks,
            required_permissions: vec![
                "Reporting: integrated report read (`open_api/v1.3/report/integrated/get/`)".into(),
                "Account Management: advertiser info & oauth2 advertiser list".into(),
                "Ad Management (可选，仅确认开启创编时): campaign/adgroup/ad create & status update".into(),
            ],
            prerequisite_steps: vec![
                "1. 在 TikTok For Business Marketing API 开发者后台创建 App 并申请 Reporting 与 Advertiser Info 权限。".into(),
                "2. 完成 OAuth 授权拿到长期 `access_token`，并记录目标 `advertiser_id`（可在账户 `extra.currency` 配置原币种如 USD，或用 `[fx_rates]` 统一折算为人民币分）。".into(),
                "3. 若企业已有海外媒体官方 MCP 鉴权通道，优先配置 `auth_strategy = \"official_mcp_bridge\"` 与 `official_mcp_endpoint` 后调用 `adlink_relabel_account_governance` 重标内部人-户关系。".into(),
            ],
            credential_fields: vec![
                spec("access_token", true, true, "TikTok Access-Token 请求头凭据", "env:TIKTOK_ADS_ACCESS_TOKEN"),
                spec("app_id", false, false, "TikTok Marketing App ID", "738910293847123"),
                spec("app_secret", false, true, "TikTok App Secret", "env:TIKTOK_ADS_APP_SECRET"),
                spec("currency", false, false, "账户原币种 ISO-4217（默认 USD）", "USD"),
            ],
            toml_snippet: r#"[connectors.tiktok_ads]
mode = "live"
access_token = "${TIKTOK_ADS_ACCESS_TOKEN}"
currency = "USD""#.into(),
            verification_steps: vec![
                "执行 `adlink sync --platforms tiktok_ads` 验证 `/report/integrated/get/` 返回的 `spend`、`complete_payment` 与 `total_complete_payment_value` 经 FX 折算后正确入库。".into(),
            ],
            gotchas: vec![
                meta.compliance_note.into(),
                "TikTok `/report/integrated/get/` `spend` 为账户原币种浮点主单位（如美元），AdLink 按 `fx_rate_to_base` 折算为人民币分并在 `raw_fields` 保留 `original_currency` 与 `original_spend`。".into(),
            ],
        },
        PlatformId::MetaAds => PlatformOnboardGuide {
            platform,
            mode: mode.into(),
            developer_portal_url: "https://developers.facebook.com/docs/marketing-apis/".into(),
            oauth_or_auth_flow: "Meta Marketing API v21.0 System User Token / OAuth2 Bearer".into(),
            optimal_login_strategy,
            fallback_login_strategy,
            ad_creation_support: meta.ad_creation_support,
            ad_creation_ask_prompt,
            ad_creation_risks,
            required_permissions: vec![
                "ads_read (`/v21.0/act_{ad_account_id}/insights` & `/me/adaccounts`)".into(),
                "read_insights (投放洞察与 actions/action_values 归因明细)".into(),
                "ads_management (可选，仅确认开启创编/改价后用于 /campaigns & /adsets 写入)".into(),
            ],
            prerequisite_steps: vec![
                "1. 在 Meta Business Manager 创建 System User 并生成永久/长效访问令牌（含 `ads_read`，如需创编加 `ads_management`）。".into(),
                "2. 记录广告账户 `act_{id}`（AdLink 会自动兼容带或不带 `act_` 前缀）。".into(),
                "3. 在账户 `extra.currency` 与 `[fx_rates]` 核对结算币种汇率（默认 USD -> 7.25 CNY）。".into(),
            ],
            credential_fields: vec![
                spec("access_token", true, true, "Meta System User / OAuth Access Token", "env:META_ADS_ACCESS_TOKEN"),
                spec("app_id", false, false, "Meta App ID", "102938475610293"),
                spec("app_secret", false, true, "Meta App Secret", "env:META_ADS_APP_SECRET"),
                spec("currency", false, false, "账户结算币种 ISO-4217（默认 USD）", "USD"),
            ],
            toml_snippet: r#"[connectors.meta_ads]
mode = "live"
access_token = "${META_ADS_ACCESS_TOKEN}"
currency = "USD""#.into(),
            verification_steps: vec![
                "执行 `adlink sync --platforms meta_ads` 验证 `spend`、`actions`（优先 `omni_purchase`）与 `action_values` 正确归一化，并测试创编预览将预算转为美分整数（cents）。".into(),
            ],
            gotchas: vec![
                meta.compliance_note.into(),
                "Meta Insights 返回 `spend` 为小数字符串（主币种元/美元），而 `/campaigns`、`/adsets` 写入 `daily_budget`/`bid_amount` 为最小货币单位整数（cents）！AdLink 已自动双向换算。".into(),
            ],
        },
        PlatformId::GoogleAds => PlatformOnboardGuide {
            platform,
            mode: mode.into(),
            developer_portal_url: "https://developers.google.com/google-ads/api/docs/start".into(),
            oauth_or_auth_flow: "Google OAuth 2.0 Bearer + developer-token (+ optional login-customer-id MCC)".into(),
            optimal_login_strategy,
            fallback_login_strategy,
            ad_creation_support: meta.ad_creation_support,
            ad_creation_ask_prompt,
            ad_creation_risks,
            required_permissions: vec![
                "https://www.googleapis.com/auth/adwords (Google Ads API v18 GAQL searchStream & listAccessibleCustomers)".into(),
                "Google Ads Manager (MCC) `developer_token`（Basic/Standard Access）与可选 `login_customer_id`".into(),
            ],
            prerequisite_steps: vec![
                "1. 通过 Google Ads MCC 账号申请并获批 `developer-token`（10 位 MCC 客户 ID 不带连字符）。".into(),
                "2. 在 Google Cloud Console 创建 OAuth 2.0 凭据（scope `https://www.googleapis.com/auth/adwords`）获取 `access_token` 与 `refresh_token`。".into(),
                "3. 若通过 MCC 代管多个子账户，配置 `login_customer_id` 后调用 `adlink_discover_platform_accounts` 一键拉取 `listAccessibleCustomers` 并完成内部人-户项目绑定。".into(),
            ],
            credential_fields: vec![
                spec("access_token", true, true, "Google OAuth 2.0 Bearer Access Token", "env:GOOGLE_ADS_ACCESS_TOKEN"),
                spec("refresh_token", false, true, "OAuth 2.0 Refresh Token", "env:GOOGLE_ADS_REFRESH_TOKEN"),
                spec("developer_token", true, true, "Google Ads API Developer Token", "env:GOOGLE_ADS_DEVELOPER_TOKEN"),
                spec("login_customer_id", false, false, "MCC 管理员账户 ID（不含横线）", "8891023456"),
                spec("currency", false, false, "账户币种（默认 USD）", "USD"),
            ],
            toml_snippet: r#"[connectors.google_ads]
mode = "live"
access_token = "${GOOGLE_ADS_ACCESS_TOKEN}"
developer_token = "${GOOGLE_ADS_DEVELOPER_TOKEN}"
login_customer_id = "8891023456"
currency = "USD""#.into(),
            verification_steps: vec![
                "执行 `adlink sync --platforms google_ads` 验证 GAQL `googleAds:searchStream` 返回的 `metrics.costMicros`（÷ 1,000,000 × FX）与 `conversionsValue` 精准入库。".into(),
            ],
            gotchas: vec![
                meta.compliance_note.into(),
                "Google Ads `costMicros` 固定以微单位（1,000,000 = 1 原币种单位）返回且创编预算也需传入 micros；AdLink 已自动完成 micros ⇄ 基准分（Fen）双向换算。".into(),
            ],
        },
        PlatformId::AppleSearchAds => PlatformOnboardGuide {
            platform,
            mode: mode.into(),
            developer_portal_url: "https://developer.apple.com/documentation/apple_ads".into(),
            oauth_or_auth_flow: "Apple Ads API v5 OAuth2 Client Credentials + Pure-Rust ES256 JWT client_secret + X-AP-Context".into(),
            optimal_login_strategy,
            fallback_login_strategy,
            ad_creation_support: meta.ad_creation_support,
            ad_creation_ask_prompt,
            ad_creation_risks,
            required_permissions: vec![
                "searchadsorg (`v5/reports/campaigns`, `/acls`, `/campaigns`, `/campaigns/{id}/adgroups`)".into(),
                "Account Admin / API Account Manager 角色授权获取 orgId".into(),
            ],
            prerequisite_steps: vec![
                "1. 在 Apple Search Ads Account Settings -> API 创建 API User，生成并下载 named_curve ECDSA P-256 PKCS#8 私钥 PEM（如 `openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:prime256v1 -pkeyopt ec_param_enc:named_curve`），并在后台上传对应公钥，获取 `client_id`（`app_id`）、`team_id`、`key_id`。".into(),
                "2. AdLink 内置纯 Rust ES256 JWT 签发器（基于 `ring::signature::EcdsaKeyPair`），无需手写脚本生成 `client_secret`，也可直接传入预先签发的 `access_token`。".into(),
                "3. 从 `/api/v5/acls` 或后台读取目标 `org_id`（用于 `X-AP-Context: orgId=...` 请求头）后即可拉取报表与账户结构。".into(),
            ],
            credential_fields: vec![
                spec("app_id", true, false, "Apple Search Ads Client ID (支持别名 client_id, SEARCHADS....)", "SEARCHADS.77b29c10-aaaa-4bbb-cccc-1234567890ab"),
                spec("team_id", false, false, "Apple Team ID / Client Secret iss", "SEARCHADS.77b29c10-aaaa-4bbb-cccc-1234567890ab"),
                spec("key_id", false, false, "Public Key ID (`kid`)", "91a82bc1-3344-5566-7788-99aabbccddee"),
                spec("es256_private_key_pem", false, true, "named_curve PKCS#8 ECDSA P-256 私钥 PEM（或直接传 access_token）", "env:APPLE_ADS_ES256_KEY_PEM"),
                spec("access_token", false, true, "预先换取的 Bearer Access Token", "env:APPLE_ADS_ACCESS_TOKEN"),
                spec("org_id", true, false, "Apple Search Ads Org ID (用于 X-AP-Context)", "4910283"),
            ],
            toml_snippet: r#"[connectors.apple_search_ads]
mode = "live"
app_id = "SEARCHADS.77b29c10-aaaa-4bbb-cccc-1234567890ab"
team_id = "SEARCHADS.77b29c10-aaaa-4bbb-cccc-1234567890ab"
key_id = "91a82bc1-3344-5566-7788-99aabbccddee"
es256_private_key_pem = "${APPLE_ADS_ES256_KEY_PEM}"
access_token = "${APPLE_ADS_ACCESS_TOKEN}"
org_id = "4910283"
currency = "USD""#.into(),
            verification_steps: vec![
                "验证 `sign_apple_es256_client_secret` 签名通过并用 ES256 客户端断言换取 OAuth token，执行 `adlink sync --platforms apple_search_ads` 检查 `localSpend.amount`、`taps`、`tapInstalls`。".into(),
            ],
            gotchas: vec![
                meta.compliance_note.into(),
                "注意：Apple Search Ads Campaign Management API v5 已公告计划于 2027 年 1 月下线，后续需平滑升级至 v6+；AdLink 已将端点版本与解析逻辑封装在独立 connector 中。".into(),
            ],
        },
        PlatformId::AmazonAds => PlatformOnboardGuide {
            platform,
            mode: mode.into(),
            developer_portal_url: "https://advertising.amazon.com/API/docs/en-us/".into(),
            oauth_or_auth_flow: "Amazon Login with Amazon (LwA) OAuth 2.0 + Amazon-Ads-ClientId + Amazon-Advertising-API-Scope".into(),
            optimal_login_strategy,
            fallback_login_strategy,
            ad_creation_support: meta.ad_creation_support,
            ad_creation_ask_prompt,
            ad_creation_risks,
            required_permissions: vec![
                "advertising::campaign_management (Profiles v2, Reporting API v3 POST/GET, Sponsored Products /sp/*)".into(),
            ],
            prerequisite_steps: vec![
                "1. 注册 Amazon Ads API Developer Portal 并通过审核获得 `client_id`（`Amazon-Ads-ClientId`）与 `client_secret`。".into(),
                "2. 完成 Login with Amazon (LwA) OAuth 授权，获取 `access_token` / `refresh_token`。".into(),
                "3. 通过 `/v2/profiles`（`adlink_discover_platform_accounts` 自动调用）枚举各站点店铺 Profile ID 并按站点配置对应币种（如 US=USD, DE=EUR, UK=GBP, JP=JPY）。".into(),
            ],
            credential_fields: vec![
                spec("access_token", true, true, "LwA OAuth2 Bearer Access Token", "env:AMAZON_ADS_ACCESS_TOKEN"),
                spec("app_id", true, false, "Amazon-Ads-ClientId", "amzn1.application-oa2-client.9876543210"),
                spec("amazon_profile_id", true, false, "Amazon-Advertising-API-Scope Profile ID", "3910294857123456"),
                spec("currency", false, false, "站点 Profile 币种（如 USD/EUR/GBP/JPY）", "USD"),
            ],
            toml_snippet: r#"[connectors.amazon_ads]
mode = "live"
access_token = "${AMAZON_ADS_ACCESS_TOKEN}"
app_id = "amzn1.application-oa2-client.9876543210"
amazon_profile_id = "3910294857123456"
currency = "USD""#.into(),
            verification_steps: vec![
                "执行 `adlink sync --platforms amazon_ads` 验证 `/reporting/reports` 异步任务提交/状态检查及 `cost`、`purchases14d`、`sales14d` 折算入库。".into(),
            ],
            gotchas: vec![
                meta.compliance_note.into(),
                "Amazon Ads Reporting v3 为异步报告且单次最大跨度 31 天（若返回 PENDING/PROCESSING，AdLink 会返回 `AsyncTaskPending` 供任务队列重试），v3 接口已公告将于 2027 年 6 月逐步迁移至统一 Reporting 接口。".into(),
            ],
        },
    }
}

fn spec(
    field: &str,
    required: bool,
    secret: bool,
    description: &str,
    example: &str,
) -> CredentialFieldSpec {
    CredentialFieldSpec {
        field: field.into(),
        required,
        secret,
        description: description.into(),
        example: example.into(),
    }
}
