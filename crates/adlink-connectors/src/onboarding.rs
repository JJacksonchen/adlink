//! Structured API onboarding walkthroughs designed to guide both human engineers
//! and AI coding/office agents (WorkBuddy, KimiWork, Claude Code, Cursor, Antigravity, Codex)
//! through connecting official ad platform APIs step by step.

use adlink_domain::PlatformId;
use adlink_engine::{CredentialFieldSpec, PlatformOnboardGuide};

/// Returns the complete onboarding guide for `platform` in `mode` (`mock` or `live`).
pub fn build_onboard_guide(platform: PlatformId, mode: &str) -> PlatformOnboardGuide {
    let meta = platform.meta();
    match platform {
        PlatformId::TencentAds => PlatformOnboardGuide {
            platform,
            mode: mode.into(),
            developer_portal_url: "https://developers.e.qq.com/".into(),
            oauth_or_auth_flow: "OAuth 2.0 Authorization Code -> access_token + refresh_token".into(),
            required_permissions: vec![
                "ads_insights (日报表/小时报/异步任务)".into(),
                "account_management (资金账户 funds/get, daily_balance_report/get)".into(),
                "ads_management (广告组查询与启停改价)".into(),
            ],
            prerequisite_steps: vec![
                "1. 在腾讯广告开发者平台创建应用并记录 client_id 与 client_secret。".into(),
                "2. 通过 OAuth2 授权页面引导广告主或代理商超级管理员完成账号授权，回调获取 authorization_code。".into(),
                "3. 换取 access_token 与 refresh_token（建议定时每 20 小时刷新一次）。".into(),
                "4. 在 AdLink config/adlink.toml 的 [connectors.tencent_ads] 配置 live 模式凭据。".into(),
            ],
            credential_fields: vec![
                spec("app_id", true, false, "腾讯广告应用 client_id", "1109283746"),
                spec("app_secret", true, true, "应用 client_secret", "env:TENCENT_ADS_SECRET"),
                spec("access_token", true, true, "OAuth2 access_token", "env:TENCENT_ADS_TOKEN"),
                spec("refresh_token", false, true, "OAuth2 refresh_token", "env:TENCENT_ADS_REFRESH"),
            ],
            toml_snippet: r#"[connectors.tencent_ads]
mode = "live"
app_id = "1109283746"
app_secret = "${TENCENT_ADS_SECRET}"
access_token = "${TENCENT_ADS_TOKEN}""#.into(),
            verification_steps: vec![
                "运行 `adlink doctor` 检查连接器健康与 token 格式。".into(),
                "调用 MCP 工具 `adlink_sync_reports` 同步近 3 天数据，确认返回 v3.0 新指标字段（cost/view_count/valid_click_count）。".into(),
            ],
            gotchas: vec![
                meta.compliance_note.into(),
                "v3.0 强制使用新指标名 view_count/valid_click_count；账面 cost 单位为分。".into(),
            ],
        },
        PlatformId::XhsJuguang | PlatformId::XhsChengfeng => PlatformOnboardGuide {
            platform,
            mode: mode.into(),
            developer_portal_url: "https://open.xiaohongshu.com/".into(),
            oauth_or_auth_flow: "OAuth 2.0 (Header: Access-Token)".into(),
            required_permissions: vec![
                "聚光/乘风报表查询 (/api/open/jg/data/report/* & /api/open/cf/data/report/*)".into(),
                "账户余额查询 (/api/open/jg/account/balance/info)".into(),
                "广告单元状态管理 (/api/open/jg/unit/update/status)".into(),
            ],
            prerequisite_steps: vec![
                "1. 联系小红书商业化对接商务或代理商管理员开通聚光/乘风开放平台白名单资质。".into(),
                "2. 在开放平台创建应用获取 app_id 与 secret，完成广告主账号 OAuth2 授权。".into(),
                "3. 写入 Access-Token 至 [connectors.xhs_juguang] / [connectors.xhs_chengfeng]。".into(),
            ],
            credential_fields: vec![
                spec("app_id", true, false, "小红书开放平台 App ID", "xhs_app_001"),
                spec("app_secret", true, true, "小红书开放平台 App Secret", "env:XHS_APP_SECRET"),
                spec("access_token", true, true, "Header Access-Token 令牌", "env:XHS_ACCESS_TOKEN"),
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
                "聚光与乘风共用底层资金账户；unit/update/status 单次最多操作 20 个单元 ID。".into(),
            ],
        },
        PlatformId::XhsPugongying | PlatformId::BilibiliHuahuo => PlatformOnboardGuide {
            platform,
            mode: mode.into(),
            developer_portal_url: meta.docs_url.into(),
            oauth_or_auth_flow: "官方认证服务商白名单 API 或 结构化 CSV/JSON 合规导入 (`POST /api/v1/ingest` / MCP `adlink_ingest_external_records`)".into(),
            required_permissions: vec![
                "品牌方/MCN认证机构商务特许接口权限（若已获官方白名单）".into(),
                "或官方后台导出的订单结算明细（含 note_id / bvid 与订单金额）".into(),
            ],
            prerequisite_steps: vec![
                "1. 注意：小红书蒲公英与B站花火无面向普通公众的免审批自助公开 API，AdLink 严格遵守合规原则，不提供任何逆向抓包脚本。".into(),
                "2. 若您的企业已获得官方服务商/品牌白名单接口权限，请在 config 中填写 partner token 与 `base_url_override`。".into(),
                "3. 若未开通白名单 API，请将官方后台导出的商单表通过 `adlink_ingest_external_records` MCP 工具或 `POST /api/v1/ingest` 导入，AdLink 会自动通过 `note_id` / `bvid` 与聚光/三连起飞打通全链路归因！".into(),
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
            oauth_or_auth_flow: "Bearer Token + X-Call-Source Header (cm.bilibili.com/takumi/api)".into(),
            required_permissions: vec![
                "B站商业化合作方三连/起飞报表读取权限".into(),
                "账户余额及计划状态管理权限".into(),
            ],
            prerequisite_steps: vec![
                "1. 向 B 站商业化对接经理申请代理商/大客户 MAPI 白名单，获取 client credentials 与分配的 `X-Call-Source` 标识。".into(),
                "2. 获取 OAuth2 access_token 并配置到 [connectors.bilibili_sanlian]。".into(),
            ],
            credential_fields: vec![
                spec("access_token", true, true, "Bearer access_token", "env:BILI_ACCESS_TOKEN"),
                spec("call_source", true, false, "B站分配的 X-Call-Source 标识", "adlink-agency-gateway"),
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
            oauth_or_auth_flow: "OAuth 2.0 (Header: Access-Token)".into(),
            required_permissions: vec![
                "数据报表 (/rest/openapi/v1/report/*)".into(),
                "广告投放管理 (/rest/openapi/v1/campaign/*)".into(),
            ],
            prerequisite_steps: vec![
                "1. 在快手磁力引擎开放平台完成企业开发者认证与 App 创建。".into(),
                "2. 通过 OAuth2 获取广告主授权 access_token 与 refresh_token。".into(),
            ],
            credential_fields: vec![
                spec("app_id", true, false, "快手开放平台 app_id", "192039"),
                spec("app_secret", true, true, "快手开放平台 secret", "env:KS_APP_SECRET"),
                spec("access_token", true, true, "Access-Token", "env:KS_ACCESS_TOKEN"),
            ],
            toml_snippet: r#"[connectors.kuaishou]
mode = "live"
app_id = "192039"
app_secret = "${KS_APP_SECRET}"
access_token = "${KS_ACCESS_TOKEN}""#.into(),
            verification_steps: vec![
                "校验 `charge`（元）被 AdLink 自动精确换算为整数分 (`Fen`)，且 `show`、`photo_click` 与原始字段同时保留。".into(),
            ],
            gotchas: vec![
                meta.compliance_note.into(),
                "实时报表接口单次跨度上限为 7 天，AdLink 引擎会自动切片并发拉取长日期区间。".into(),
            ],
        },
        PlatformId::OceanEngineAd | PlatformId::OceanEngineLocal => PlatformOnboardGuide {
            platform,
            mode: mode.into(),
            developer_portal_url: "https://open.oceanengine.com/".into(),
            oauth_or_auth_flow: "OAuth 2.0 (Header: Access-Token)".into(),
            required_permissions: vec![
                "ocean_engine.reports.query (/open_api/v3.0/report/custom/get/)".into(),
                "本地推报表权限 (/open_api/v3.0/local/report/project/get/)".into(),
                "账户日流水 (/open_api/2/advertiser/fund/daily_stat/)".into(),
            ],
            prerequisite_steps: vec![
                "1. 登录巨量引擎开放平台创建应用并勾选升级版 v3.0 报表与本地推权限点。".into(),
                "2. 通过 OAuth2 授权回调获取 access_token（有效期 24 小时，refresh_token 30 天）。".into(),
            ],
            credential_fields: vec![
                spec("app_id", true, false, "巨量开放平台 App ID", "178293840192"),
                spec("app_secret", true, true, "App Secret", "env:OCEAN_APP_SECRET"),
                spec("access_token", true, true, "OAuth2 Access-Token", "env:OCEAN_ACCESS_TOKEN"),
            ],
            toml_snippet: format!(
                "[connectors.{}]\nmode = \"live\"\napp_id = \"178293840192\"\napp_secret = \"${{OCEAN_APP_SECRET}}\"\naccess_token = \"${{OCEAN_ACCESS_TOKEN}}\"",
                platform.as_str()
            ),
            verification_steps: vec![
                "拉取 `report/custom/get/` 验证 `stat_cost`(元→分)、`show_cnt`、`click_cnt`、`convert_cnt` 准确入库。".into(),
            ],
            gotchas: vec![
                meta.compliance_note.into(),
                "巨量资金日流水返回单位为元，AdLink 在边界统一转分存储以保证对账零误差。".into(),
            ],
        },
        PlatformId::AlipayDenghuo => PlatformOnboardGuide {
            platform,
            mode: mode.into(),
            developer_portal_url: "https://open.alipay.com/".into(),
            oauth_or_auth_flow: "Alipay OpenAPI RSA2 (SHA256WithRSA) + biz_token + alipay_pid".into(),
            required_permissions: vec![
                "支付宝灯火广告投放数据查询 (alipay.data.dataservice.ad.reportdata.query)".into(),
                "灯火消费明细查询 (alipay.data.dataservice.ad.consumehistory.query)".into(),
            ],
            prerequisite_steps: vec![
                "1. 登录支付宝开放平台创建应用并绑定“支付宝广告投放（灯火）”能力包。".into(),
                "2. 生成 PKCS#8 RSA2 2048位密钥对，在开放平台上传应用公钥，将私钥保存至安全文件路径或环境变量。".into(),
                "3. 从灯火代理商/商家后台获取 `biz_token` 与 `2088` 开头的 18 位 `alipay_pid`。".into(),
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
                "执行 `adlink sync --platforms alipay_denghuo` 自动按 7 天切片并将日期转为 `yyyyMMdd`。".into(),
            ],
            gotchas: vec![
                meta.compliance_note.into(),
                "品牌广告不返回消耗或消耗为0；效果广告返回 cost(分) 与 conv_data_list。".into(),
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
