# 创意类型分类、内部创作工作量与跨平台种草归因

## 1. 创意分类体系 (`CreativeTaxonomy`)
每条素材记录在 `creative_assets` 中均可维护：
- **形式 (`format`)**：`short_video` | `live_clip` | `image_text_note` | `carousel` | `splash` | `playable` | `kol_custom`
- **钩子类型 (`hook_type`)**：`pain_point` | `contrast` | `unboxing` | `story` | `promo_price` | `expert_authority` | `lifestyle`
- **场景/卖点/调性 + 任意自定义标签**：`scene_tag`、`selling_point`、`tone`、`custom_tags`

## 2. 内部创作工作量与投放数据归因打通 (`CreativeProductionContribution`)
- 每条创意资产记录内部 `production_hours` 与 `production_cost_fen`。
- 通过 `CreativeProductionContribution` 记录每位参与人员（`scriptwriter` 编导、`shooter` 拍摄、`editor` 剪辑、`designer` 设计、`ai_operator` 等）的 **`work_hours`** 与 **`credit_weight`**（如 0.30 / 0.25 / 0.30 / 0.15）。
- 调用 `adlink_creative_attribution_report` 或 `POST /api/v1/attribution/creatives` 后，系统自动产出：
  - `gmv_per_work_hour_yuan`：创作者每投入 1 小时带来的归因 GMV
  - `spend_per_work_hour_yuan`：创作者每小时支撑的媒体消耗规模
  - `avg_attributed_roi`：创作者历史素材平均归因 ROI

## 3. 跨平台内容种草 + 付费加热归因 (`note_id` / `bvid`)
- 小红书生态：蒲公英商单 (`xhs_pugongying`) 与聚光 (`xhs_juguang`) / 乘风 (`xhs_chengfeng`) 通过 **`note_id`** 自动串联。
- B站生态：花火商单 (`bilibili_huahuo`) 与三连起飞 (`bilibili_sanlian`) 通过 **`bvid`** 自动串联。
- 归因报告自动输出 `seeding_boost_attribution`，给出「达人商单费 + 平台服务费 + 信息流加热消耗」打包后的 **`full_funnel_cpe_yuan`** 与 **`full_funnel_roi`**。
