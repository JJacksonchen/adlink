---
name: adlink-ad-middleware
description: Operate AdLink enterprise ad-data middleware — onboard official Chinese ad platform APIs (Tencent, XHS Juguang/Chengfeng/Pugongying, Bilibili Sanlian/Huahuo, Kuaishou, Ocean Engine AD/Local, Alipay Denghuo), reconcile spend/performance/custom calibers, attribute creative work-hours, and execute guarded ad mutations.
---

# AdLink Skill for AI Agents

When helping a user operate or deploy **AdLink**:

1. **Always check live state first** with `adlink_health_check` or `GET /api/v1/health`.
2. **To connect live media accounts**:
   - Call `adlink_onboard_guide` for each requested `platform` (`tencent_ads`, `xhs_juguang`, `xhs_chengfeng`, `xhs_pugongying`, `bilibili_sanlian`, `bilibili_huahuo`, `kuaishou`, `ocean_engine_ad`, `ocean_engine_local`, `alipay_denghuo`).
   - Guide the user to store secrets in environment variables referenced by `${VAR}` inside `config/adlink.local.toml`.
   - For `xhs_pugongying` and `bilibili_huahuo`, explain honestly that no open public self-serve MAPI exists; guide them to either configure a whitelisted partner gateway or ingest official exports via `adlink_ingest_external_records` preserving `note_id` / `bvid`.
3. **Always clarify metric calibers**:
   - Distinguish `gross_media_cost_yuan` (账面消耗), `cash_cost_yuan` (现金消耗), `net_after_rebate_cost_yuan` (返点后净消耗), `full_funnel_cost_yuan` (全域成本), `agency_net_revenue_yuan` (代理商净营收), `platform_gmv_yuan` (平台 GMV), and user-defined custom calibers via `adlink_reconcile_calibers` and `adlink_define_custom_caliber`.
4. **Enforce safety on ad mutations**:
   - Always preview `adlink_ad_mutate` with `dry_run: true` and supply an explicit `reason` before requesting confirmation to run `dry_run: false`.
