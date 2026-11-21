# 8 大主流媒体官方 API 字段映射与口径对照表

所有平台连接器在解析官方 JSON 响应时，一方面严格按照下表映射为统一权威类型（金额统一以 `Fen(i64)` 分存储），另一方面完整保留原始 JSON 键值于 `unified_reports.raw_fields_json`，确保业务侧不丢失任何特有指标。

## 1. 腾讯社交广告 (`tencent_ads`, Marketing API v3.0)
- **接口**：`GET /v3.0/daily_reports/get`, `GET /v3.0/hourly_reports/get`, `GET /v3.0/funds/get`, `GET /v3.0/daily_balance_report/get`
- **日期窗口**：单次最长 90 天（建议每日 08:00 后同步 D-1 全量数据）
- **字段映射**：`cost` (分) → `gross_cost_fen`；`view_count` → `impressions`；`valid_click_count` → `clicks`；`conversions_count` → `conversions`；`cheout_fd` → `gmv_fen`；`time_line` (`REPORTING_TIME` / `REQUEST_TIME`) → `time_line`

## 2. 小红书聚光 & 乘风 (`xhs_juguang`, `xhs_chengfeng`)
- **接口**：`https://adapi.xiaohongshu.com` (`Access-Token`), `/api/open/jg/data/report/offline/*`, `/api/open/cf/data/report/offline/*`, `/api/open/jg/account/balance/info`, `/api/open/jg/unit/update/status`
- **日期窗口**：单次最长 30 天
- **字段映射**：`fee` (分) → `gross_cost_fen`；`impression` → `impressions`；`click` → `clicks`；`interaction` → `interactions`；`deal_order_num` → `orders`；`deal_order_gmv` (分) → `gmv_fen`；`note_id` → `content_id`

## 3. 小红书蒲公英 (`xhs_pugongying`) & B站花火 (`bilibili_huahuo`)
- **接入说明**：两平台暂无面向普通开发者的公开免审批自助 API。AdLink 支持（1）认证白名单合作网关配置，与（2）通过 `POST /api/v1/ingest` 或 MCP `adlink_ingest_external_records` 导入官方导出的结算明细。
- **字段映射**：
  - 蒲公英：`order_id` → `entity_id`；`note_id` → `content_id`；`kol_quote_fee` → `kol_order_fee_fen`；`platform_service_fee` → `platform_service_fee_fen`；`read_cnt` → `impressions`；`like_cnt+collect_cnt+comment_cnt` → `interactions`
  - 花火：`huahuo_order_id` → `entity_id`；`bvid` → `content_id`；`order_amount` → `kol_order_fee_fen`；`platform_fee` → `platform_service_fee_fen`；`blue_link_click_cnt` → `clicks`

## 4. B站三连 / 商业起飞 (`bilibili_sanlian`)
- **接口**：`https://cm.bilibili.com/takumi/api` (`Authorization: Bearer` + `X-Call-Source`)
- **字段映射**：`cost` (分) → `gross_cost_fen`；`impression` → `impressions`；`click` → `clicks`；`play_cnt` → `video_plays`；`like_cnt+coin_cnt+fav_cnt+share_cnt` → `interactions`；`bvid` → `content_id`

## 5. 快手磁力引擎 (`kuaishou`)
- **接口**：`https://ad.e.kuaishou.com/rest/openapi/v1/report/{account_report,campaign_report,unit_report,creative_report}`
- **日期窗口**：单次最长 7 天（`DateRangeChunker` 自动切片）
- **字段映射**：`charge` (**元** → `*100` 转 `Fen`) → `gross_cost_fen`；`show` → `impressions`；`photo_click` → `clicks`；`event_pay_purchase_amount` (**元** → `Fen`) → `gmv_fen`；`photo_id` → `content_id`

## 6. 巨量引擎 AD & 巨量本地推 (`ocean_engine_ad`, `ocean_engine_local`)
- **接口**：`https://api.oceanengine.com/open_api/v3.0/report/custom/get/`, `/open_api/2/advertiser/fund/daily_stat/`, `/open_api/v3.0/local/report/project/get/`, `/open_api/v3.0/local/report/promotion/get/`
- **日期窗口**：单次最长 30 天
- **字段映射**：`stat_cost` (**元** → `Fen`) → `gross_cost_fen`；`show_cnt` → `impressions`；`click_cnt` → `clicks`；`convert_cnt` → `conversions`；`pay_order_amount` (**元** → `Fen`) → `gmv_fen`；`local_life_verify_cnt` → `store_verifies`；`poi_id`/`material_id` → `content_id`

## 7. 支付宝灯火 (`alipay_denghuo`)
- **接口**：`https://openapi.alipay.com/gateway.do` (`method=alipay.data.dataservice.ad.reportdata.query`, **纯 Rust RSA2 SHA256WithRSA 签名**)
- **日期窗口**：`DETAIL` 最长 7 天、`HOUR` 最长 1 天，日期格式 `yyyyMMdd`
- **字段映射**：`cost` (分) → `gross_cost_fen`；`impression` → `impressions`；`click` → `clicks`；`conv_result` / `conv_data_list` → `conversions`/`leads`/`orders`
