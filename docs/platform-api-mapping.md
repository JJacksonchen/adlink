# 8 大主流媒体官方 API 逐字段核对报告、双层登录架构与创编能力矩阵

> 本文件依据各媒体官方开放平台文档/SDK 规范逐项核对 **AdLink 请求参数、响应解析字段、货币单位、前置维度/主题约束** 与 **广告创编开放情况**，对公开文档未开放或受行业/白名单限制的字段作如实标注，绝不虚构或掩盖限制。
>
> 注：所有连接器均在规范化为 `UnifiedReportRow` 的同时，将平台原始 JSON 行完整保存于 `unified_reports.raw_fields_json`。

---

## 一、8 大平台逐字段核对状态图例

| 状态标识 | 含义 |
| :--- | :--- |
| ✅ **公开官方核验可直接拉取** | 已核对官方公开 Marketing API / SDK 参数与返回字段名，实时请求可直接拉取 |
| ⚠️ **受前置参数/转化事件约束** | 官方要求特定 `group_by`、`data_topic`、`ad_level` 或转化事件 `conv_code_list` 方可返回，AdLink 已自动适配或补齐请求参数 |
| 🔀 **派生聚合或兼容别名** | 通过多官方互动子字段求和，或同时兼容官方短字段名与详情长字段名 |
| 🔒 **合作方白名单 NDA 字段** | 官方未向公众免审批全量开放接口白皮书，已按合作文档约定对齐并支持 `base_url_override` / `extra` 适配 |
| 📥 **无公开自助 MAPI（合规接入）** | 官方不面向普通公众开放免审批自助拉数 API，严格通过白名单网关或官方导出文件 `POST /api/v1/ingest` 接入 |

---

## 二、逐平台字段核对与修复清单

### 1. 腾讯社交广告 (`tencent_ads`, Marketing API v3.0)
- **拉数接口**：`GET /v3.0/daily_reports/get`、`GET /v3.0/hourly_reports/get`、`GET /v3.0/funds/get`、`GET /v3.0/advertiser/get`
- **本次审计修复点**：
  1. **修复漏传 `group_by` 参数**：官方 AMS v3.0 不传 `group_by` 时仅返回账户级汇总；现已按请求层级自动传递 `["date"]`、`["date","campaign_id"]`、`["date","adgroup_id"]`、`["date","adgroup_id","dynamic_creative_id"]`（小时报追加 `"hour"`）。
  2. **对齐 `fields` 与首日付费计数**：请求参数按层级补全 `adgroup_name`、`cheout_pv_1d`、`web_leads_count`、`praise_count`、`comment_count`、`video_play_count`、`video_inner_play_count`；解析器优先取 `cheout_pv_1d` 并兼容 `cheout_fd_count`；数字型 `campaign_id`/`material_id` 改用 `json_opt_str` 避免丢失。
  3. **唯一 `nonce`**：每次请求使用随机 UUID `nonce` + `timestamp`。

| 规范字段 | 腾讯官方字段名 | 官方单位 | 实时拉取条件与诚实说明 | 核验状态 |
| :--- | :--- | :--- | :--- | :--- |
| `stat_date` / `stat_hour` | `date` / `hour` | 日期 / 0-23 | 小时报需传 `group_by` 含 `"hour"` | ✅ |
| `entity_id` | `account_id` / `campaign_id` / `adgroup_id` / `dynamic_creative_id` | ID | AMS v3.0 主投放链路为「广告组+动态创意」两层 | ✅ |
| `entity_name` | `adgroup_name` / `campaign_name` | string | `adgroup_name` 需匹配广告组及以上明细层级 | ⚠️ |
| `gross_cost_fen` | `cost` | **分** | 直接按整数分存储 | ✅ |
| `cash_cost_fen` / `grant_cost_fen` | `funds/get` (`fund_type`, `balance`, `realtime_cost`) | **分** | 日结账面拆现金/赠款，另有 `daily_balance_report/get` 可配 | ✅ |
| `impressions` / `clicks` | `view_count` / `valid_click_count` | count | v3.0 强制新指标命名 | ✅ |
| `conversions` / `deep_conversions` | `conversions_count` / `deep_conversions_count` | count | 支持 `REPORTING_TIME` / `REQUEST_TIME` | ✅ |
| `leads` | `web_leads_count` / `leads_user_count` | count | 依广告优化目标而定，非全行业均有值 | ⚠️ |
| `orders` / `gmv_fen` | `cheout_pv_1d` / `cheout_fd` | count / **分** | 首日 1d 归因窗；付费类广告返回，曝光类为 0 | ⚠️ |
| `interactions` / `video_plays` | `praise_count + comment_count` / `video_play_count` | count | 原生视频/朋友圈/视频号版位返回 | ⚠️ |

---

### 2. 巨量引擎 AD & 巨量本地推 (`ocean_engine_ad`, `ocean_engine_local`, v3.0)
- **拉数接口**：`GET /open_api/v3.0/report/custom/get/`、`GET /open_api/v3.0/local/report/project/get/`、`GET /open_api/v3.0/local/report/promotion/get/`、`GET /open_api/2/advertiser/fund/daily_stat/`、`GET /open_api/oauth2/advertiser/get/`
- **本次审计修复点**：
  1. **修复创意层 `data_topic` 错误**：官方 v3.0 请求 `material_id` 维度时不能传 `BASIC_DATA`，否则直接报错；现已对 `Creative`/`ContentOrder` 自动切换 `"data_topic": "MATERIAL_DATA"`，且统一传入 `start_time`/`end_time` (`YYYY-MM-DD`) 与 `stat_time_hour`（小时粒度）。
  2. **补全 `metrics` 请求清单与资金实际拆分**：请求参数补齐 `dy_like`、`dy_comment`、`dy_follow`、`total_play`、`valid_play`；`daily_stat` 优先读取实际 `cash_cost` 与 `grant_cost`/`valid_grant_cost`，仅在未返回时按 `default_cash_ratio` 估算。
  3. **时间解析兼容字符串小时**：解析 `stat_time_hour` 兼容 `"2026-11-10 14:00:00"` 与整型 `14`；`project_id`/`material_id`/`poi_id` 兼容数字 ID。

| 规范字段 | 巨量官方字段名 | 官方单位 | 实时拉取条件与诚实说明 | 核验状态 |
| :--- | :--- | :--- | :--- | :--- |
| `stat_date` / `stat_hour` | `stat_time_day` / `stat_time_hour` | 日期 / 小时 | 本地推部分接口传 `start_date`/`end_date` | ✅ |
| `entity_id` | `advertiser_id` / `project_id` / `promotion_id` / `material_id` | ID | `material_id` 必须配 `data_topic=MATERIAL_DATA` | ⚠️ |
| `gross_cost_fen` | `stat_cost` | **元 (float)** | 边界 `*100` 精确转分 (`Fen`) | ✅ |
| `impressions` / `clicks` | `show_cnt` / `click_cnt` | count | `BASIC_DATA` 与 `MATERIAL_DATA` 通用 | ✅ |
| `conversions` / `leads` | `convert_cnt` / `clue_cnt` (`form_cnt`) | count | `clue_cnt` 依线索目标开启状态返回 | ⚠️ |
| `orders` / `gmv_fen` | `pay_order_count` / `pay_order_amount` | count / **元** | 电商/直播/本地推团购交易返回，已精确转分 | ⚠️ |
| `interactions` / `video_plays` / `new_follows` | `dy_like + dy_comment` / `total_play`, `valid_play` / `dy_follow` | count | 抖音生态视频广告返回，`MATERIAL_DATA` 子集略有精简 | ⚠️ |
| `store_verifies` | `local_life_verify_cnt` | count | 仅 **巨量本地推** 核销类商品返回 | ⚠️ |

---

### 3. 快手磁力引擎 (`kuaishou`, Marketing API v1)
- **拉数接口**：`POST /rest/openapi/v1/report/{account_report,campaign_report,unit_report,creative_report}`、`POST /v1/agent/advertiser/list`
- **本次审计修复点**：
  1. **对齐官方真实响应字段名**：clicks 同时兼容 `photo_click` / `aclick` / `click_count`；leads 兼容 `form_count` / `submit` / `submit_form_count`；orders 兼容 `event_pay` / `event_pay_purchase_count`；conversions 兼容 `activation` / `activation_count`；`entity_name` 按层级检查 `campaign_name` / `unit_name` / `creative_name`。
  2. **修复翻页滞留第一页 Bug**：部分快手接口仅返回 `data.total_count` 而不返回 `has_more`；现已按 `page * page_size < total_count` 自动计算 `has_more`。
  3. **货币单位严正区分**：报表 `charge` / `event_pay_purchase_amount` 为**元**，而快手创编接口 `day_budget` / `bid` 为**厘 (`1 Fen = 10 厘`)**，已在创编实现内明确 `*10` 换算。

| 规范字段 | 快手官方字段名 | 官方单位 | 实时拉取条件与诚实说明 | 核验状态 |
| :--- | :--- | :--- | :--- | :--- |
| `stat_date` / `stat_hour` | `stat_date` / `stat_hour` | 日期 / 小时 | 日期切片上限 7 天，`DateRangeChunker` 自动切片 | ✅ |
| `entity_id` / `entity_name` | `campaign_id` / `unit_id` / `creative_id` + `*_name` | ID / string | 数字/字符串 ID 均兼容 | ✅ |
| `gross_cost_fen` | `charge` | **元 (Double)** | 自动 `*100` 精确转 `Fen` | ✅ |
| `impressions` / `clicks` | `show` / `photo_click`, `aclick` | count | 同时兼容素材点击 `aclick` | ✅ |
| `leads` / `orders` / `conversions` | `form_count`, `submit` / `event_pay` / `activation` | count | 支持短字段与详细字段别名 | ✅ |
| `gmv_fen` | `event_pay_purchase_amount` | **元 (Double)** | 自动转 `Fen` | ✅ |
| `interactions` / `video_plays` | `like + comment` / `play_3s` | count | 视频播放 3s 指标 | ✅ |

---

### 4. 小红书聚光 & 乘风 (`xhs_juguang`, `xhs_chengfeng`)
- **拉数接口**：`POST https://adapi.xiaohongshu.com/api/open/{jg,cf}/data/report/offline/{account,campaign,unit,creative}`、`POST /api/open/jg/account/balance/info`、`POST /api/open/jg/account/list`
- **本次审计修复点**：
  1. **修复元字符串/分整数解析歧义**：部分白名单文档/账户返回的报表 `fee` 与 `deal_order_gmv` 为保留两位小数的元字符串（如 `"123.45"`）而非整数分；现引入 `json_money_fen`（自动识别小数点并支持 `extra.report_money_unit = "yuan"` 配置），防止静默归零或放大 100 倍！
  2. **补齐线索字段与分页推导**：兼容 `leads` / `clue_cnt` / `form_submit_cnt` 线索字段；增加 `total_count` 翻页推导。
  3. **如实声明访问范围**：小红书完整《聚光开放平台推广白皮书》需合作方白名单登录后查看全部枚举，建议实盘上线时用 3 天小窗口验证实际返回体。

| 规范字段 | 小红书字段名 | 单位与适配 | 实时拉取条件与诚实说明 | 核验状态 |
| :--- | :--- | :--- | :--- | :--- |
| `stat_date` / `entity_id` | `time` (`date`) / `campaign_id`, `unit_id`, `creative_id` | 日期 / ID | 余额接口已公开证实金额为分，报表金额自动兼容元/分 | 🔒 |
| `gross_cost_fen` | `fee` | **分 / 元字符串** | `json_money_fen` 自动识别带小数元字符串与整数分 | ⚠️ |
| `impressions` / `clicks` / `interactions` | `impression` / `click` / `interaction` | count | 种草互动总计数 | 🔒 |
| `leads` / `orders` / `gmv_fen` | `leads`/`clue_cnt` / `deal_order_num` / `deal_order_gmv` | count / 分或元 | `deal_order_num`/`deal_order_gmv` 仅**乘风电商闭环**具备 | ⚠️ |
| `content_id` | `note_id` | string | 关联蒲公英种草笔记的核心主键 | 🔒 |

---

### 5. B站三连 / 商业起飞 (`bilibili_sanlian`)
- **拉数接口**：`https://cm.bilibili.com/takumi/api`（Headers: `Authorization: Bearer`、`X-Call-Source`，可选 `extra.skills_version` -> `X-Sanlian-Skills-Version`）
- **审计诚实说明**：B站商业起飞/三连 MAPI 属于商业合作方 NDA 接口，未向公众无门槛公开全部字段字典。连接器支持 `base_url_override`、`extra.skills_version`、多层级 `entity_name` fallback 及 `total` 分页推算，并在 `raw_fields` 无损保留所有字段。

---

### 6. 小红书蒲公英 (`xhs_pugongying`) & B站花火 (`bilibili_huahuo`)
- **接入状态**：📥 **两平台无面向普通公众的免审批自助公开 MAPI**。
- **合规落地路径**：已获得品牌/MCN官方白名单者可在 config 中填写 `base_url_override` + `access_token`；否则通过 `POST /api/v1/ingest` 或 MCP `adlink_ingest_external_records` 导入官方后台导出的订单表（务必保留 `note_id` / `bvid`），AdLink 自动完成商单费+服务费与聚光/乘风/三连起飞的跨平台全链路归因。

---

### 7. 支付宝灯火 (`alipay_denghuo`)
- **拉数接口**：`https://openapi.alipay.com/gateway.do`（纯 Rust `ring` RSA2 SHA256WithRSA 签名，`alipay.data.dataservice.ad.reportdata.query`）
- **本次审计修复的重大 Bug**：
  1. **修复致命字段拼写不一致**：支付宝官方 `AdReportDataDetail` 实际返回的转化明细数组键名为 **`conversion_data_list`**（原代码仅检查了错误的 `conv_data_list`），已修正为优先读取 `conversion_data_list`（并兼容 `conv_data_list`），同步匹配 `conversion_type_name` 中文名称识别表单/订单转化！
  2. **遵循层级限制**：官方文档明确当 `ad_level` 为 `PRINCIPAL` / `MARKET_TARGET` 时不返回 `conversion_data_list`（仅 `PLAN` / `GROUP` / `CREATIVE` 等层级返回），请求构造已据此自动适配，并支持配置 `extra.conv_code_list` 指定转化指标 Code。
  3. **如实收敛创编与修改能力**：核对支付宝开放平台后确认，灯火公开 OpenAPI 提供报表查询、转化回传 (`conversion.upload`) 与落地页管理 (`landinginfo.createormodify`)，**并未公开自助建计划/单元/创意或启停改预算 OpenAPI**；已在代码中将 `ad_creation_support` 与 `supports_ad_mutation` 如实标为公开不支持。

| 规范字段 | 支付宝官方字段名 | 官方单位 | 实时拉取条件与诚实说明 | 核验状态 |
| :--- | :--- | :--- | :--- | :--- |
| `stat_date` / `stat_hour` | `biz_date` (`yyyyMMdd`) / `biz_hour` | 日期 / 小时 | `DETAIL` 最长 7 天，`HOUR` 最长 1 天 | ✅ |
| `entity_id` / `entity_name` | `data_id` / `data_name` | string | `PRINCIPAL` 层级 `data_id` 为空，自动回落为 `alipay_pid` | ✅ |
| `gross_cost_fen` | `cost` (`cost_format` 为元) | **分** | 效果广告返回整型分，品牌广告无消耗或为 0 | ✅ |
| `impressions` / `clicks` / `conversions` | `impression` / `click` / `conv_result` | count | `reportdata.query` 核心返回字段 | ✅ |
| `leads` / `orders` / `gmv_fen` | `conversion_data_list[]` (`conversion_type`, `conversion_type_name`, `conversion_result`) | count / 视配置 | 需非 `PRINCIPAL` 层级 + `show_conv_data=Y` + 配置了对应 `conv_code_list` 才可提取交易/线索细分 | ⚠️ |

---

## 三、双层登录鉴权架构与媒体创编 (`创编`) 能力总表

| 平台 ID | 最优登录方案 (Tier 1) | 降级登录方案 (Tier 2) | 创编能力级别 (`ad_creation_support`) | 官方创编端点与单位换算注意 |
| :--- | :--- | :--- | :--- | :--- |
| `tencent_ads` | 复用官方 MCP 登录鉴权 + `adlink_relabel_account_governance` 重标人户项目 | `GET /v3.0/advertiser/get` 拉账户 + `adlink_fallback_bootstrap_project` 建项目 | `official_public_api` | `POST /v3.0/adgroups/add`, `POST /v3.0/dynamic_creatives/add`（v3.0 已合并计划为 2 层；单位：**分**） |
| `ocean_engine_ad` | 同上 (`official_mcp_bridge` + 重标) | `GET /open_api/oauth2/advertiser/get/` + 建项目绑子户 | `official_public_api` | `POST /open_api/v3.0/project/create/`, `POST /open_api/v3.0/promotion/create/`（单位：**元**, `Fen -> float`） |
| `ocean_engine_local` | 同上 (`official_mcp_bridge` + 重标) | `GET /open_api/oauth2/advertiser/get/` + 建项目绑子户 | `official_public_api` | `POST /open_api/v3.0/local/project/create/`, `POST /open_api/v3.0/local/promotion/create/`（单位：**元**） |
| `kuaishou` | 同上 (`official_mcp_bridge` + 重标) | `POST /v1/agent/advertiser/list` + 建项目绑子户 | `official_public_api` | `POST /v1/campaign/create`, `POST /v2/ad_unit/create`, `POST /v2/creative/create`（注意：**创编用「厘」=`Fen*10`**） |
| `xhs_juguang` / `xhs_chengfeng` | 同上 (`official_mcp_bridge` + 重标) | `POST /api/open/jg/account/list` + 建项目绑子户 | `official_partner_api` | `POST /api/open/jg/campaign/create`, `/unit/create`, `/creativity/create`（需白名单；单位：**分**） |
| `bilibili_sanlian` | 同上 (`official_mcp_bridge` + 重标) | `GET /takumi/api/account/list` + 建项目绑子户 | `official_partner_api` | 合作方 NDA 创编接口（支持 `base_url_override`） |
| `alipay_denghuo` | 复用官方 MCP（若有）或 PID 登记 + 重标 | 配置 `app_id` + RSA2 私钥 + `biz_token` + `alipay_pid` | `not_supported_publicly` | 公开 OpenAPI 不提供自助计划/单元/创意创编及改价接口 |
| `xhs_pugongying` / `bilibili_huahuo` | 白名单网关或结构化导入 + 重标项目 | 官方导表 `/api/v1/ingest` + `note_id`/`bvid` 关联 | `not_supported_publicly` | KOL 达人商单平台无公开自助创编 API |
