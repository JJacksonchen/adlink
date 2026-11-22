# AdLink — 广告投放数据底座与 AI-Native MCP 中间件

**简体中文** | [English](README.en.md)

[![CI](https://github.com/zyfjacksonchen-source/adlink/actions/workflows/ci.yml/badge.svg)](https://github.com/zyfjacksonchen-source/adlink/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-Workspace-orange.svg)](Cargo.toml)
[![Platforms](https://img.shields.io/badge/Platforms-15%20(CN%20%2B%20Global)-brightgreen.svg)](docs/platform-api-mapping.md)

> **数字化是 AI Native 的前提。** AdLink（Apache-2.0 开源可商用）是一个用 **Rust** 构建的高性能、高度解耦的广告数据治理与 MCP 中间件，打通 **国内 10 大连接器 + 海外 5 大主流平台（共 15 个平台连接器）**，支持多币种 FX 折算与企业内部治理、多端 AI Agent 生态以及高并发商业化生产服务端架构。

[English README](README.en.md) · [Agent API 接入引导手册 (中文)](AGENT_GUIDE.md) · [Agent Guide (EN)](AGENT_GUIDE.en.md) · [Agent Skill (SKILL.md)](SKILL.md) · [官方 API 逐字段核对表](docs/platform-api-mapping.md) · [架构设计](docs/architecture.md) · [插件二开指南](docs/plugin-development.md)

---

## 1. 项目目的与行业意义

广告主与代理商在推进 **FDE（Forward Deployed Engineer / 商业数字化交付）与 AI 化转型** 时，往往卡在最底层的工程现实：
1. **媒体协议碎片化、字段与币种不统一**：国内（腾讯、巨量、快手、小红书、B站、支付宝）与海外（TikTok、Meta、Google、Apple Search Ads、Amazon）各平台单位迥异（分 / 元 / 厘 / `micros` / `cents` + USD/EUR/GBP/JPY 多币种）、时间跨度上限不同（1天/7天/30天/31天/90天）、鉴权机制复杂（OAuth2 / `Bearer + X-Call-Source` / 支付宝 RSA2 / Google `developer-token` / Apple `ES256` P-256 JWT）。
2. **只拉流水进不了生产环境**：没有理清 **人-户关系（含生效期与分摊权重）**、**项目类型分类**、**创意类型分类与内部创作者工时归因**、以及 **消耗口径 / 业绩口径 / 自定义口径** 的数据，AI Agent 拿到的只是一堆无法对账的脏表。
3. **跨 Agent 重复造轮子**：WorkBuddy、KimiWork、TraeWork、豆包工作、千问办公、Claude Code、Codex、Antigravity、Cursor、GrokBot、Muse、Dots 以及自建 Agent 需要统一、可控、带安全护栏的 MCP 底座。

---

## 2. 为什么媒体平台已有官方 MCP，企业还需要 AdLink？

> **核心一句话**：官方平台 MCP 只解决了「单个媒体接口能不能被 AI 调通」；**AdLink 解决的是「多家媒体原始数据」与「企业高度定制化人-户关系、项目归属、财务口径、创意工时归因等内部管理体系」之间的连接问题，彻底免去每个团队重复开发定制中间件的高昂成本。**

| 维度 | 仅接各家媒体官方 MCP | 使用 **AdLink 企业底座 + 官方 MCP** |
| :--- | :--- | :--- |
| **登录与鉴权成本** | 每家单独做 OAuth、Token 刷新、密钥管理 | **双层策略（最优直接复用官方 MCP 登录鉴权接口，降级直连 API 拉取账户列表）** |
| **人-户与项目归属** | 仅见媒体账户 ID，不知归属哪个优化师/项目/客户 | **支持自由重标「人-户关系」（含分摊权重/生效期）与项目归类，或一键自建项目绑定子账户** |
| **多平台财务与口径** | 各报各的单位（分/元/厘），无返点/服务费/赠款概念 | **统一精确转 `Fen`，区分账面/现金/净消耗/全域成本与代理商净营收，支持自定义 AST 口径** |
| **创意与内部创作归因** | 缺乏企业自建文案钩子分类与编导/剪辑工时核算 | **全量归因 `gmv_per_work_hour_yuan`，并打通 `note_id`/`bvid` 种草+付费加热闭环** |
| **写操作与创编安全** | 裸露创建/改价接口，Agent 易误操作烧预算 | **创编默认关闭 (`enabled=false`)，默认询问用户并强提醒 5 类风险；强制 `PAUSED` + `dry_run` + 硬上限审计** |

---

### 双层登录鉴权架构（官方 MCP 复用优先 + API 降级兜底）

```mermaid
flowchart LR
    A["接入媒体账号 / 开始初始化"] --> B{"媒体侧是否有官方 MCP?"}
    B -- "是 · 最优方案 (Tier 1)" --> C["直接调用官方 MCP 登录鉴权接口<br/>(`auth_strategy = official_mcp_bridge`)"]
    C --> D["`adlink_discover_platform_accounts`<br/>同步已鉴权账户列表"]
    D --> E["`adlink_relabel_account_governance`<br/>在 AdLink 自由重标人-户关系与项目归属"]
    B -- "否 · 降级方案 (Tier 2)" --> F["直连媒体 OpenAPI 拉取账户/子账户列表<br/>(`auth_strategy = direct_api_account_discovery`)"]
    F --> G["`adlink_fallback_bootstrap_project`<br/>自行创建企业项目并一键关联选中子账户与优化师"]
    E --> H["统一口径对账 · 创意归因 · 24 个通用 MCP 工具"]
    G --> H
```

---

## 3. 适用人群

| 角色 | 核心诉求 | AdLink 提供的核心能力 |
| :--- | :--- | :--- |
| **品牌/广告主 FDE 与 AI 转型负责人** | 搭建企业投放数据基座，让办公/代码 Agent 安全查数与调价 | 一站式容器部署 + MCP 三协议接入 + 安全护栏（默认 `dry_run` + 预算封顶 + 审计日志） |
| **广告代理商技术 / 运营 / 媒介负责人** | 理清优化师/媒介人-户绑定、返点、服务费、现金/赠款、项目毛利 | 人户关系时效分摊 + 项目类型分类 + 消耗/业绩/自定义口径自动对账 |
| **创意/内容团队负责人** | 量化编导/拍摄/剪辑/设计工时投入与投放 ROI，打通种草+加热 | 创意分类打标 + `note_id`/`bvid` 跨平台归因 + 创作者单小时产出 GMV |
| **广告媒体研发人员** | 快速对接国内外官方 Marketing API、长日期并发拉数与二次开发 | 官方字段映射零丢失保留 (`raw_fields`) + 纯 Rust 支付宝 RSA2 & Apple ES256 JWT 签名 + 插件化二开 |

---

## 4. 核心架构图 (DeepSeek-Harness / Cordis 微内核思路)

```mermaid
flowchart TB
    subgraph Agents["多端 AI Agents & 客户端层"]
        A1["国内办公 Agent: WorkBuddy · KimiWork · TraeWork · 豆包工作 · 千问办公"]
        A2["国际编程 Agent: Claude Code · Codex · Antigravity · Cursor"]
        A3["Personal & 自建 Agent: GrokBot · Muse · Dots · Dify/Coze"]
        A4["用户自定义前端: Web 控制台 (web-starter/) · 微信/UniApp 小程序 SDK"]
    end

    subgraph Transport["统一协议、防护与高并发入口层 (adlink-mcp + adlink-api)"]
        T1["MCP Stdio · Streamable HTTP (/mcp) · Legacy SSE (/sse)"]
        T2["Headless REST /api/v1/* · 异步入队 /sync/jobs/enqueue (202) · OpenAPI 3.1"]
        T3["自适应过载保护 (503) · 租户令牌桶限流 (429) · 超时控制 (504) · 幂等 (Idempotency-Key)"]
        T4["Epoch 失效分析缓存 (X-Cache) · Prometheus (/metrics) · K8s 探针 (/healthz /readyz)"]
        T5["Bearer / X-Api-Key 鉴权中间件 & 请求追踪 X-Request-Id"]
    end

    subgraph Kernel["微内核插件总线 (adlink-kernel · Cordis 理念)"]
        K1["PluginContext 作用域依赖注入 (ServiceRegistry)"]
        K2["可逆副作用栈 (EffectScope 自动回滚)"]
        K3["HookPipeline: BeforeApiRequest / AfterNormalizeRow / BeforeAdMutate / BeforeAdCreate"]
        K4["GuardrailPlugin: 创编默认关闭与询问 · 风险确认 · 预算/出价上限 · 审计记录"]
    end

    subgraph Engine["高并发引擎与 WAL 存储池 (adlink-engine + adlink-domain + adlink-storage)"]
        E1["日期切片器 DateRangeChunker (1d/7d/30d/31d/90d) & 分级令牌桶 RateLimiterPool"]
        E2["断点续传 SyncCoordinator · 多 Worker 异步队列 · Webhook 回调"]
        E3["多币种 FX 折算引擎 FxRateTable & 安全公式 AST (消耗/业绩/自定义口径)"]
        E4["SQLite WAL 读写分离连接池 (单写通道 write_epoch + N 并发只读池 + 停机 Checkpoint)"]
    end

    subgraph Connectors["15 大国内外媒体官方连接器插件层 (adlink-connectors)"]
        C1["国内: 腾讯 AMS v3.0 · 小红书聚光/乘风/蒲公英"]
        C2["国内: B站三连/花火 · 快手磁力 · 巨量 AD/本地推 · 支付宝灯火"]
        C3["海外: TikTok v1.3 · Meta v21.0 · Google v18 · Apple Search Ads v5 (ES256) · Amazon v3"]
    end

    Agents --> Transport
    Transport --> Kernel
    Kernel --> Engine
    Engine --> Connectors
```

### 企业数据治理模型（人-户关系 · 项目分类 · 创意归因）

```mermaid
erDiagram
    ORGANIZATION ||--o{ PERSON : employs
    ORGANIZATION ||--o{ AD_ACCOUNT : owns
    ORGANIZATION ||--o{ PROJECT : manages
    PERSON ||--o{ PERSON_ACCOUNT_BINDING : "分摊权重 + 生效区间"
    AD_ACCOUNT ||--o{ PERSON_ACCOUNT_BINDING : bound_to
    PROJECT ||--o{ PROJECT_ACCOUNT_LINK : allocates
    AD_ACCOUNT ||--o{ PROJECT_ACCOUNT_LINK : linked
    PROJECT ||--o{ CREATIVE_ASSET : contains
    CREATIVE_ASSET ||--o{ CREATIVE_PLATFORM_LINK : "entity_id / note_id / bvid"
    CREATIVE_ASSET ||--o{ CREATIVE_PRODUCTION_CONTRIBUTION : "工时 + 功劳权重"
    PERSON ||--o{ CREATIVE_PRODUCTION_CONTRIBUTION : creates
    AD_ACCOUNT ||--o{ UNIFIED_REPORT : generates
    AD_ACCOUNT ||--o{ DAILY_FUND_STATEMENT : reconciles
```

---

## 5. 支持的 15 大国内外主流媒体平台与官方字段对齐

> 逐字段实时拉取核验（含多币种 FX 折算、请求参数约束、创编接口支持级别与官方下线预告）：参见 **[docs/platform-api-mapping.md](docs/platform-api-mapping.md)**。

### 国内主流平台（10 个连接器）

| 平台 | 连接器 ID | 接入性质 | 官方基础域名 / 协议 | 货币单位归一化 | 最大单次切片 | 核心官方字段保留 |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **腾讯社交广告** | `tencent_ads` | 公开 MAPI v3.0 | `https://api.e.qq.com/v3.0` (OAuth2) | 分 -> `Fen` | 90 天 | `cost`, `view_count`, `valid_click_count`, `conversions_count`, `cheout_fd`, `cheout_pv_1d`, `group_by` |
| **小红书聚光** | `xhs_juguang` | 合作方 OpenAPI | `https://adapi.xiaohongshu.com` (`Access-Token`) | 分/元字符串 -> `Fen` | 30 天 | `fee`, `impression`, `click`, `interaction`, `note_id`, `convert_cnt` |
| **小红书乘风** | `xhs_chengfeng` | 合作方 OpenAPI | `https://adapi.xiaohongshu.com` (`Access-Token`) | 分/元字符串 -> `Fen` | 30 天 | `fee`, `impression`, `click`, `deal_order_num`, `deal_order_gmv`, `note_id` |
| **小红书蒲公英** | `xhs_pugongying` | 白名单 / 导表合规打通 | Partner API 或 `/api/v1/ingest` | 分 -> `Fen` | 30 天 | `order_id`, `note_id`, `kol_quote_fee`, `platform_service_fee`, `read_cnt`, `like_cnt`, `collect_cnt` |
| **B站三连/起飞** | `bilibili_sanlian` | 合作方 MAPI | `https://cm.bilibili.com/takumi/api` (`Bearer` + `X-Call-Source`) | 分 -> `Fen` | 30 天 | `cost`, `impression`, `click`, `play_cnt`, `like_cnt`, `coin_cnt`, `fav_cnt`, `bvid` |
| **B站花火平台** | `bilibili_huahuo` | 白名单 / 导表合规打通 | Partner API 或 `/api/v1/ingest` | 分 -> `Fen` | 30 天 | `huahuo_order_id`, `bvid`, `order_amount`, `platform_fee`, `blue_link_click_cnt`, `gmv` |
| **快手磁力引擎** | `kuaishou` | 公开 MAPI v1 | `https://ad.e.kuaishou.com/rest/openapi` (`Access-Token`) | 报表**元**/创编**厘** -> `Fen` | 7 天 | `charge`, `show`, `photo_click`/`aclick`, `form_count`/`submit`, `event_pay`, `event_pay_purchase_amount`, `photo_id` |
| **巨量引擎 AD** | `ocean_engine_ad` | 公开 MAPI v3.0 | `https://api.oceanengine.com/open_api/v3.0` (`Access-Token`) | **元 -> 精确转分** | 30 天 | `stat_cost`, `show_cnt`, `click_cnt`, `convert_cnt`, `pay_order_count`, `pay_order_amount`, `material_id` (`MATERIAL_DATA`) |
| **巨量本地推** | `ocean_engine_local` | 公开 MAPI v3.0 | `https://api.oceanengine.com/open_api/v3.0/local` | **元 -> 精确转分** | 30 天 | `stat_cost`, `show_cnt`, `click_cnt`, `pay_order_amount`, `poi_id`, `local_life_verify_cnt` |
| **支付宝灯火** | `alipay_denghuo` | 公开 OpenAPI | `https://openapi.alipay.com/gateway.do` (**纯 Rust RSA2**) | 分 -> `Fen` | 7 天 (小时1天) | `cost`, `impression`, `click`, `conv_result`, `conversion_data_list`, `biz_date` |

### 海外主流平台（5 个连接器 · 多币种 `[fx_rates]` 自动折算基准 CNY `Fen`）

| 平台 | 连接器 ID | 官方 API 版本 | 鉴权与关键请求头 | 单位换算与官方注意 | 最大单次切片 | 核心官方字段保留 |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **TikTok Ads** | `tiktok_ads` | Marketing API `v1.3` | `Access-Token` | 原币种浮点 × FX -> `Fen` | 30 天 | `spend`, `impressions`, `clicks`, `complete_payment`, `total_complete_payment_value` |
| **Meta Ads (FB/IG)** | `meta_ads` | Graph API `v21.0` | `Authorization: Bearer` | Insights 主币种字符串；创编自动转**最小整数分 (cents)**；优先 `omni_purchase` | 30 天 | `spend`, `impressions`, `clicks`, `actions[]`, `action_values[]` |
| **Google Ads** | `google_ads` | Google Ads API `v18` | `Bearer` + `developer-token` (+ MCC `login-customer-id`) | `costMicros` (`÷ 1,000,000` × FX -> `Fen`) | 90 天 | `metrics.costMicros`, `metrics.conversions`, `metrics.conversionsValue` |
| **Apple Search Ads** | `apple_search_ads` | Ads API `v5` | `Bearer` (**纯 Rust ES256 P-256 JWT**) + `X-AP-Context` | `localSpend.amount` × FX -> `Fen`（⚠️ v5 计划 2027-01 下线） | 30 天 | `localSpend`, `impressions`, `taps`, `tapInstalls` |
| **Amazon Ads** | `amazon_ads` | Ads API `v3` | `Bearer` + `Amazon-Ads-ClientId` + `Scope` Profile | 异步报告 `PENDING` 返回 `AsyncTaskPending`（⚠️ v3 预计 2027-06 迁移） | 31 天 | `cost`, `purchases14d`, `sales14d`, `advertisedAsin` |

> **合规与完整性承诺**：小红书蒲公英与 B站花火目前均无公开免审批自助 API，AdLink **严禁任何违规爬虫或绕过风控手段**，诚实采用「官方白名单接口 + 结构化结算导出打通」双路径，所有原始字段（含海外平台 `original_currency`、`fx_rate_to_base`、`original_spend`）均无损留存于 `unified_reports.raw_fields_json`。

---

## 6. 高并发商业化生产服务端架构与一部署即用

除了核心连接器与 MCP 工具外，AdLink `adlink-storage` + `adlink-api` + `adlink-cli` 按照高并发商业化生产标准实现：
- **SQLite WAL 读写分离连接池**：单序列化写入通道（提交后递增 O(1) `write_epoch`）+ 可配置只读池（默认 `reader_pool_size = 8`，开启 `query_only = ON` 与 `mmap_size = 256MB`），高并发查询与批量写入零互斥阻塞。
- **Epoch 失效多级分析缓存 (`AnalyticsQueryCache`)**：对 `/api/v1/analytics/query`、`/api/v1/analytics/reconcile`、`/api/v1/attribution/creatives` 提供毫秒级缓存命中（响应头 `X-Cache: HIT|MISS`），任意新数据写入或入库立即按 `write_epoch` 自动失效。
- **幂等去重 (`Idempotency-Key`)**：`/api/v1/ingest`、`/api/v1/sync/jobs/enqueue`、`/api/v1/ad-ops/mutate`、`/api/v1/ad-creation/create` 支持请求幂等重放保护（响应头 `X-Idempotency-Replayed: true`）。
- **自适应过载保护与分级限流**：信号量并发上限（`max_in_flight_requests` 饱和返回 `503 Service Unavailable` + `Retry-After`）、每租户/IP 令牌桶限流（`429 Too Many Requests`）、请求超时（`504`，自动豁免 `/sse` 长连接）、`DefaultBodyLimit` 载荷限制、`X-Request-Id` 与安全响应头。
- **有界异步同步工作队列 + Webhook + 定时调度**：`POST /api/v1/sync/jobs/enqueue` 返回 `202 Accepted`，多 Worker 后台执行并通过 `GET /api/v1/sync/jobs/{id}` 查询进度；可选 `webhook_url` 回调与 `scheduled_sync_interval_secs` 自动增量拉数。
- **云原生可观测与优雅停机**：提供 `GET /healthz`（存活性探针）、`GET /readyz`（就绪探针，收到 `SIGTERM`/`SIGINT` 后立即切为 `503` 摘流）、`GET /metrics`（Prometheus 文本指标：QPS、耗时直方图、缓存命中率、队列深度、WAL `write_epoch`）、JSON 结构化 `stderr` 日志，停机时自动排空工作队列并执行 `PRAGMA wal_checkpoint(TRUNCATE)`。

## 7. 一站式快速安装与生产部署

### 方式 A：源码编译运行（5 分钟体验全量模拟数据）

```bash
git clone https://github.com/zyfjacksonchen-source/adlink.git
cd adlink

# 编译 Release 二进制
cargo build --release -p adlink-cli

# 启动一体化服务（REST API + Streamable HTTP /mcp + Legacy SSE /sse + Web 控制台）
# 自动播种「星图智投」全套人户关系/项目分类/创意归因及 30 天国内外 15 平台模拟数据
./target/release/adlink serve --config config/adlink.mock.toml --seed-mock
```

启动后访问：
- **Web 控制台**：`http://127.0.0.1:8787/`
- **MCP Streamable HTTP**：`http://127.0.0.1:8787/mcp`
- **MCP Legacy SSE**：`http://127.0.0.1:8787/sse`
- **云原生探针与监控**：`http://127.0.0.1:8787/healthz` · `/readyz` · `/metrics`
- **OpenAPI 3.1 规范**：`http://127.0.0.1:8787/api/v1/openapi.json`
- **Schema-Driven UI 契约**：`http://127.0.0.1:8787/api/v1/ui/schema`

### 方式 B：Docker / Nginx / Kubernetes 商业生产部署

```bash
cp config/adlink.example.toml config/adlink.local.toml
# 单机 Docker
docker compose up -d --build

# 商业生产架构（Nginx Keepalive/SSE 代理 + /metrics 内网隔离 + /healthz 健康检查）
docker compose -f docker-compose.prod.yml up -d --build

# Kubernetes StatefulSet（含 PVC WAL 持久化、/readyz 摘流、Prometheus 自动抓取）
kubectl apply -f deploy/k8s-deployment.yaml
```

### 常用 CLI 命令

```bash
# 诊断配置、SQLite WAL、内核插件与连接器状态
adlink doctor --config config/adlink.mock.toml

# 并发回溯 180 天历史数据
adlink sync --config config/adlink.mock.toml --start 2026-05-01 --end 2026-10-27 --concurrency 16

# 一键生成任意客户端 MCP 配置 (workbuddy/kimiwork/traework/doubao/qianwen/claude-code/codex/antigravity/cursor/grokbot/muse/dots)
adlink mcp-config --client workbuddy
adlink mcp-config --client codex
```

---

## 8. 国内外主流 Agent MCP 适配矩阵

AdLink 同时支持 **Stdio**、**Streamable HTTP (2025/2026 有状态 & 无状态)**、**Legacy SSE**，开箱适配：

| 客户端类型 | 代表产品 | 推荐接入模式 | 现成配置模板 |
| :--- | :--- | :--- | :--- |
| **国内办公/工作台 Agent** | WorkBuddy、KimiWork、TraeWork、豆包工作、千问办公 | Streamable HTTP (`/mcp`) / Legacy SSE (`/sse`) / Stdio | [streamable-http-clients.json](config/agents/streamable-http-clients.json) |
| **海外主流 AI 编程工具** | Claude Code、Codex、Antigravity、Cursor | `adlink mcp-stdio` 或 `/mcp` | [stdio-desktop-clients.json](config/agents/stdio-desktop-clients.json) · [codex.toml](config/agents/codex.toml) |
| **Personal Agent** | GrokBot、Muse、Dots | Streamable HTTP (`/mcp`) / Stdio | `adlink mcp-config --client grokbot` |
| **企业自建 Agent / 编排** | Dify、Coze、FastGPT、LangGraph、自研 Agent | `/mcp` 或 `/api/v1/openapi.json` | [openapi.rs](crates/adlink-api/src/openapi.rs) |

### 引导用户 Agent 连接真实媒体 API

部署完成后，您的 AI Agent 可直接读取项目内置的 **[AGENT_GUIDE.md](AGENT_GUIDE.md)** / **[AGENT_GUIDE.en.md](AGENT_GUIDE.en.md)** 与 **[SKILL.md](SKILL.md)**，或调用 MCP 工具（共 **24 个工具**）：
1. `adlink_platform_catalog` & `adlink_onboard_guide` — 获取全部 15 个平台逐字段映射、认证步骤与创编能力说明。
2. `adlink_discover_platform_accounts` — 最优走官方 MCP 鉴权桥接、降级走媒体 API 直接拉取授权子账户列表。
3. `adlink_relabel_account_governance`（重标人-户-项目关系）与 `adlink_fallback_bootstrap_project`（降级自建项目关联子账户）。

### 广告创编（`[ad_creation]`）：默认关闭 · 主动询问 · 风险强提醒

AdLink 已接入多平台广告创编能力（`adlink_creation_status_and_risks`、`adlink_ad_create` 与 `POST /api/v1/ad-creation/create`），但出于企业资金与合规安全原则，**默认恒为关闭**：
- **默认行为**：`[ad_creation]` 中 `enabled = false`、`risk_acknowledged = false`；未授权调用创编时直接拦截并要求 Agent **先询问用户『是否需要接入广告创编（新建计划/单元/创意）能力？』**。
- **强制 5 项风险提示**：（1）真实资金消耗与跨平台单位/币种差异（快手创编用**厘**、腾讯/小红书用**分**、巨量与 TikTok 用主单位**元**、Meta 创编用 **cents**、Google 用 **micros**）；（2）素材/文案审核拒审与账户封禁风险；（3）定向/落地页/归因错配风险；（4）媒体架构与白名单差异（腾讯 AMS v3.0 两层架构、小红书/B站需白名单、支付宝灯火/蒲公英/花火无公开自助创编 API）；（5）即便显式开启，仍强制 `initial_status = "PAUSED"` 暂停态创建、强制先过 `dry_run = true` 预览、受预算/出价硬上限保护并全量写入不可变审计日志。

详见逐字段核对表：[docs/platform-api-mapping.md](docs/platform-api-mapping.md)。

---

## 9. 口径治理与创作工作量归因体系

- **消耗口径（Spend Calibers）**：
  - `账面总消耗` = 现金消耗 + 赠款消耗
  - `现金消耗` = 仅计算真实现金扣款（对照日结资金流水）
  - `返点后净消耗` = 账面消耗 × `(1 - rebate_rate)`
  - `全域投入成本` = 账面消耗 + 达人商单费 + 平台服务费 + 内部创作成本
- **业绩口径（Performance Calibers）**：
  - `代理商毛流水`、`代理商净营收`（媒体返点 + 客户服务费）、`平台回传 GMV`、`品牌净 ROI`
- **自定义口径（Custom AST DSL）**：支持安全的纯 Rust AST 公式解析，例如：
  ```text
  safe_div(gmv_yuan, net_after_rebate_yuan + kol_order_fee_yuan + platform_service_fee_yuan)
  ```
- **内部创作工作量与投放数据打通**：支持为每条创意记录编导/拍摄/剪辑/设计的 `work_hours` 与 `credit_weight`，自动聚合出每个创作者的 `gmv_per_work_hour_yuan`（单小时产出 GMV）和 `spend_per_work_hour_yuan`；同时通过 `note_id`（蒲公英+聚光/乘风）与 `bvid`（花火+三连起飞）实现种草+付费加热闭环归因。

---

## 10. 网页与小程序多端自定义 (Headless + Schema-Driven UI)

AdLink 内置零构建依赖的 `web-starter/` 控制台与 `web-starter/miniprogram-sdk/adlink-mp.js` 小程序 SDK：
- 更换自定义前端只需将您的 React / Vue / Next.js 构建产物目录通过 `--web-dir ./my-dist` 传入即可；
- 前端通过 `GET /api/v1/ui/schema` 动态获取口径选项、导航菜单与平台列表，实现真正的后端解耦多端定制。

---

## 11. 开源协议

本项目采用 **[Apache License 2.0](LICENSE)** 开源，可放心用于商业交付、企业自建及二次开发。详见 [LICENSE](LICENSE) 与 [NOTICE](NOTICE)。
