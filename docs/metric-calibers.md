# 消耗口径 · 业绩口径 · 自定义口径治理规范

企业投放进入生产环境后，财务、运营、媒介和品牌客户使用的"消耗"与"业绩"往往不是同一个数字。AdLink 内置标准口径与安全公式 AST 引擎，所有分维度查询均可一键输出完整对账明细（`CaliberReconciliation`）。

## 1. 标准消耗口径 (`SpendCaliber`)

| 口径字段 | 中文定义 | 计算逻辑 | 适用场景 |
| :--- | :--- | :--- | :--- |
| `gross_media_cost_yuan` | 媒体账面消耗 | `cash_cost + grant_cost` | 优化师跑量日报、媒体账单核对 |
| `cash_cost_yuan` | 现金消耗 | 实际现金池扣款（不含赠款/返货券） | 财务资金核销、真实现金流评估 |
| `net_after_rebate_cost_yuan` | 返点后净消耗 | `gross_media_cost × (1 - account.rebate_rate)` | 品牌方实际净投放成本核算 |
| `full_funnel_cost_yuan` | 全域投入成本 | `gross_media_cost + kol_order_fee + platform_service_fee + production_cost` | 种草+加热+创作全链路利润核算 |

## 2. 标准业绩口径 (`PerformanceCaliber`)

| 口径字段 | 中文定义 | 计算逻辑 | 适用场景 |
| :--- | :--- | :--- | :--- |
| `agency_gross_billing_yuan` | 代理商毛流水 | `gross_media_cost × (1 + service_fee_rate)` | 代理商开票营收口径 |
| `agency_net_revenue_yuan` | 代理商净营收 | `gross_media_cost × rebate_rate + gross_media_cost × service_fee_rate` | 代理商团队绩效考核 |
| `platform_gmv_yuan` | 平台回传 GMV | 各平台成交回传总金额 | 电商投放与本地推核销 |

## 3. 自定义口径 (`CustomCaliberDef` 安全公式 DSL)

通过 `adlink_define_custom_caliber` 或 `POST /api/v1/calibers` 创建自定义公式。解析器采用纯 Rust 递归下降 AST（无任何 eval/shell 风险，内置除零保护）：
- 支持运算符：`+`, `-`, `*`, `/`, `>`, `>=`, `<`, `<=`, `==`
- 支持函数：`safe_div(a, b)`, `min(a, b)`, `max(a, b)`, `abs(x)`, `round(x, digits)`, `if(cond, t, f)`
- 可用变量：`gross_cost_yuan`, `cash_cost_yuan`, `grant_cost_yuan`, `kol_order_fee_yuan`, `platform_service_fee_yuan`, `production_cost_yuan`, `gmv_yuan`, `rebate_yuan`, `agency_fee_yuan`, `net_after_rebate_yuan`, `full_funnel_cost_yuan`, `agency_gross_billing_yuan`, `agency_net_revenue_yuan`, `impressions`, `clicks`, `interactions`, `conversions`, `leads`, `orders`, `store_verifies`
