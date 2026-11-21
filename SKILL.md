---
name: adlink-ad-middleware
description: Operate AdLink enterprise ad-data middleware — bridge isolated official ad-platform APIs/MCPs with customized enterprise person-account governance, project classification, caliber reconciliation, creative workload attribution, opt-in ad creation (disabled by default with mandatory user ask & risk warning), and two-tier login (Official MCP Auth Bridge + relabeling, or API account list + custom project bootstrap).
---

# AdLink Skill for AI Agents

Why AdLink even when ad platforms offer official MCPs: AdLink bridges raw single-platform APIs/MCPs and **highly customized internal enterprise management** (`人-户关系`, `项目分类`, `消耗/业绩/自定义口径`, `创意工时归因`), dramatically cutting custom FDE engineering & maintenance cost.

When helping a user operate or deploy **AdLink**:

1. **Always check live state & creation policy first** with `adlink_health_check` and `adlink_creation_status_and_risks`.
2. **Use the Two-Tier Login / Auth Strategy**:
   - **Optimal (Tier 1)**: If the platform already provides an Official MCP authentication endpoint, reuse it (`auth_strategy = "official_mcp_bridge"`), call `adlink_discover_platform_accounts`, and call `adlink_relabel_account_governance` to freely re-label (`重标`) `人-户关系` and `项目归属` inside AdLink.
   - **Fallback (Tier 2)**: If using headless Marketing API credentials, guide secret env vars inside `config/adlink.local.toml`, call `adlink_discover_platform_accounts` to fetch the authorized sub-account list, and call `adlink_fallback_bootstrap_project` to create a custom project and link selected sub-accounts + owners.
   - **Compliance for `xhs_pugongying` & `bilibili_huahuo`**: Explain honestly that no open public self-serve MAPI exists; guide whitelisted gateway setup or `adlink_ingest_external_records` CSV/JSON import preserving `note_id` / `bvid`.
3. **MANDATORY Ad Creation (`创编`) Safety Protocol**:
   - `ad_creation.enabled` is **`false` by default**.
   - **ALWAYS ask the user** whether they want to opt into Ad Creation (`创编`) AND disclose all 5 financial/policy/unit-conversion risks returned by `adlink_creation_status_and_risks`.
   - Never recommend enabling `[ad_creation]` without explicit user confirmation of the risks. Even once enabled (`enabled = true`, `risk_acknowledged = true`), ALWAYS run `adlink_ad_create` with `dry_run: true` first to inspect `platform_payload_preview` before passing `preview_confirmed: true, dry_run: false`.
4. **Always clarify metric calibers**:
   - Reconcile `gross_media_cost_yuan` (账面消耗), `cash_cost_yuan` (现金消耗), `net_after_rebate_cost_yuan` (返点后净消耗), `full_funnel_cost_yuan` (全域成本), `agency_net_revenue_yuan` (代理商净营收), `platform_gmv_yuan` (平台 GMV), and user-defined custom calibers via `adlink_reconcile_calibers` and `adlink_define_custom_caliber`.
5. **Enforce safety on ad mutations**:
   - Always preview `adlink_ad_mutate` with `dry_run: true` and an explicit `reason` before user-confirmed `dry_run: false`.
