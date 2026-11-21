# AdLink AI Agent API 接入引导手册 (AGENT_GUIDE)

> 本指南专门写给接入 AdLink 的 **AI Agent（WorkBuddy、KimiWork、TraeWork、豆包工作、千问办公、Claude Code、Codex、Antigravity、Cursor、GrokBot、Muse、Dots 等）** 与 **FDE 实施工程师**。
> 当 AdLink 一站式部署完成后，请按以下标准化 SOP 引导用户完成真实媒体官方 API 的绑定、校验与上线。

---

## 标准引导 SOP（供 Agent 执行）

### 第一步：探测当前环境与账户现状
1. 调用 `adlink_health_check` 确认内核、SQLite WAL 数据库以及 10 个连接器的当前运行状态（`mock` 或 `live`）。
2. 调用 `adlink_platform_catalog` 向用户确认其本次实际投放的平台范围。

### 第二步：按平台拉取官方接入清单并逐项引导用户授权
对用户需要连接的每个平台 `P`，调用：
- `adlink_onboard_guide(platform: P)`（或触发 Prompt `onboard_ad_platform(platform: P)`）

向用户展示以下 4 项结构化内容，不要遗漏任何权限点与安全提示：
1. **官方开发者门户与授权方式**（如 OAuth2 授权回调 / 支付宝 RSA2 PKCS#8 密钥 + `biz_token` + `alipay_pid` / B站 `X-Call-Source`）。
2. **必须申请的官方权限 Scope**（例如腾讯 `ads_insights` + `account_management` + `ads_management`；巨量 `ocean_engine.reports.query` + 本地推权限）。
3. **环境变量与配置注入**：提醒用户**永远不要将真实密钥硬编码进 git**，指导其将密钥写入 `.env`，并在 `config/adlink.local.toml` 中通过 `${ENV_VAR}` 引用。
4. **特别合规说明（蒲公英与花火）**：
   - 如用户使用 `xhs_pugongying` 或 `bilibili_huahuo`，主动告知：**这两个平台官方目前不提供公开自助 MAPI**。
   - 询问用户是否已获品牌/MCN 官方合作白名单网关；若无白名单，直接指导用户把官方后台导出的结算/订单表转为 JSON 调用 `adlink_ingest_external_records`（或 `POST /api/v1/ingest`），并确保带上 `note_id`（小红书）或 `bvid`（B站），AdLink 会自动与聚光/乘风/三连起飞打通全链路归因！

### 第三步：写入人-户关系与项目归类（进生产前必做）
连接账号之后，调用以下工具把企业治理信息补充完整：
1. `adlink_connect_account`：登记账户的 `rebate_rate`（媒体返点率）、`service_fee_rate`（服务费率）、`default_cash_ratio`。
2. `adlink_manage_org_people(action: "bind_person_account")`：登记优化师/媒介/AM 的**生效日期 (`effective_from`) 与分摊权重 (`split_weight`)**。
3. `adlink_manage_projects`：创建项目并设置准确的 `project_type`（如 `ecommerce_gmv` / `seeding_content` / `local_life` / `performance_acquisition`）。

### 第四步：小窗口冒烟验证与长日期回溯
1. 先调用 `adlink_sync_reports` 同步近 **3 天**数据进行冒烟校验，检查是否存在 401/403 权限不足或字段缺失。
2. 再并发同步历史长周期（例如 90~180 天），最后调用 `adlink_reconcile_calibers` 与客户财务导出后台对账核验 `gross_media_cost_yuan` 与 `cash_cost_yuan`。
3. 涉及广告关停或调预算时，必须先用 `dry_run: true` 预览并在获取用户明确确认后再设为 `false`。
