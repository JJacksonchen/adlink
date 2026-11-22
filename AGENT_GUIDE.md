# AdLink AI Agent API 接入引导手册 (AGENT_GUIDE)

> 本指南专门写给接入 AdLink 的 **AI Agent（WorkBuddy、KimiWork、TraeWork、豆包工作、千问办公、Claude Code、Codex、Antigravity、Cursor、GrokBot、Muse、Dots 等）** 与 **FDE 实施工程师**。
>
> **核心定位提醒**：当各媒体平台已经推出各自官方 MCP 时，**AdLink 解决的是「媒体原始数据」与「企业高度定制化人-户关系、项目归属、财务口径、内部创作工时归因」之间的连接问题，避免在每个客户现场重复写死胶水代码，大幅降低研发与长期维护成本。**

---

## 标准引导 SOP（供 Agent 执行）

### 第一步：探测环境、官方 MCP 状况与创编开关状态
1. 调用 `adlink_health_check` 确认内核、SQLite WAL 读写池（含 `write_epoch`）以及全部 **15 个国内外连接器**（国内 10 个 + 海外 `tiktok_ads`、`meta_ads`、`google_ads`、`apple_search_ads`、`amazon_ads`）的当前运行状态（`mock` 或 `live`）。
2. 调用 `adlink_creation_status_and_risks` 检查当前 `[ad_creation]` 开关状态（**默认恒为 `enabled = false`**），获取官方要求的用户询问话术与 5 项风险提醒。

### 第二步：双层登录与账户发现（最优复用官方 MCP + 降级直连兜底）
对用户需要连接的每个平台 `P`，先调用 `adlink_onboard_guide(platform: P)` 与 `adlink_discover_platform_accounts(platform: P)`：

1. **最优方案（Tier 1 — 直接复用官方 MCP 登录鉴权接口 + 重标人户项目关系）**：
   - 若用户已经在使用的媒体平台提供了**官方 MCP Server**（自带 OAuth 登录/账户鉴权），无需重做登录页，只需在 `[connectors.<platform>]` 配置 `auth_strategy = "official_mcp_bridge"` 与 `official_mcp_endpoint`。
   - 调用 `adlink_discover_platform_accounts` 拉取官方 MCP 已鉴权的广告主账户及当前 AdLink 映射情况。
   - 调用 **`adlink_relabel_account_governance`**，把真实人-户关系（含优化师、分摊权重 `split_weight`、生效期）和项目归属（含 `project_type`、`campaign_filter_regex`）直接在 AdLink 中自由**重标**。

2. **降级方案（Tier 2 — 拉取账户列表 + 自行创建项目关联子账户）**：
   - 若媒体平台暂未提供官方 MCP 或客户需要服务端无头对接，引导用户配置开放平台 API 凭据（永远将密钥放入 `.env`，通过 `${ENV_VAR}` 注入 `config/adlink.local.toml`）：
     - 海外平台补充凭据：`google_ads` 需 `developer_token`（及可选 MCC `login_customer_id`）；`apple_search_ads` 需 `app_id`/`team_id`/`key_id` + `named_curve` P-256 PKCS#8 `es256_private_key_pem` + `org_id`；`amazon_ads` 需 `app_id`（`Amazon-Ads-ClientId`）+ `amazon_profile_id`。
     - 海外多币种账户可通过 `extra.currency` / `[fx_rates]` 将 USD/EUR/GBP/JPY 统一折算为人民币基准 `Fen`。
   - 调用 `adlink_discover_platform_accounts` 拉取授权子账户列表。
   - 引导用户选定子账户，调用 **`adlink_fallback_bootstrap_project`** 一键创建自定义项目并关联选中子账户与负责优化师。

3. **特别合规说明（小红书蒲公英与 B 站花火）**：
   - 这两个平台**无面向公众的免审批自助 MAPI**（亦禁止任何爬虫抓包）。
   - 引导已获官方白名单的用户配置 `base_url_override`，其余用户把官方导出商单表通过 `adlink_ingest_external_records`（`POST /api/v1/ingest`）导入，务必保留 `note_id`（小红书）或 `bvid`（B站）以打通种草+加热闭环归因。

---

### 第三步：创编接入默认询问与强安全护栏（MANDATORY RULE）

> **强制规则**：在任何引导接入、运维或新建计划场景下，AI Agent **必须默认保持「只读拉数 + Dry-Run 运维」模式，禁止未经询问直接建议或开启写创编权限**：

1. **必须先主动询问用户**：
   > 「是否需要接入广告创编（通过 API 新建推广计划 / 单元 / 创意）能力？」
2. **如果用户考虑接入，必须完整提示以下 5 项创编风险**：
   - **【资金与单位换算风险】**：创编会直接在媒体端产生可消耗资产，且快手创编用「厘」(`1元=1000厘`)、腾讯/小红书用「分」、巨量用「元」，极易因小数点或参数失误导致预算瞬时超标消耗。
   - **【审核拒审与封户风险】**：AI 自动生成文案/素材如触犯广告法或行业违禁词，可能导致计划拒审、主体扣分甚至账户封禁。
   - **【定向与归因错配风险】**：定向错误、落地页 URL 或转化归因像素错配会导致无效预算浪费。
   - **【平台差异与白名单】**：腾讯 AMS v3.0 只有「广告组+动态创意」两层；小红书/B站创编需官方白名单；支付宝灯火及蒲公英/花火无公开自助创编 API。
   - **【开启后强制安全约束】**：需用户书面确认后方可在 `config/adlink.toml` 设置 `[ad_creation] enabled = true` 且 `risk_acknowledged = true`；每次调用 `adlink_ad_create` 仍强制默认 `initial_status = "PAUSED"`（暂停态建出）、必须先过 `dry_run: true` 输出官方报文预览、并在用户确认后带 `preview_confirmed: true` 方可真建。

---

### 第四步：小窗口冒烟验证、高并发异步入队与口径对账
1. 先调用 `adlink_sync_reports` 同步近 **3 天**数据进行冒烟校验，核对关键指标与货币换算；针对超大规模历史回溯可通过商业服务端 `POST /api/v1/sync/jobs/enqueue` 异步入队（`202 Accepted`），并轮询 `GET /api/v1/sync/jobs/{id}` 或配置 `webhook_url` 接收回调（服务端同时暴露 `/healthz`、`/readyz`、`/metrics`）。
2. 再并发同步长周期历史数据（如 90~180 天），调用 `adlink_reconcile_calibers` 对齐账面消耗、现金消耗、返点后净消耗与代理商净营收 / 客户自定义口径。
3. 任何启停调预算 (`adlink_ad_mutate`) 一律先 `dry_run: true` 预览并在用户确认后执行。
