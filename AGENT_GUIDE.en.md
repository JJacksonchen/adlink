# AdLink AI Agent API Onboarding Playbook (AGENT_GUIDE)

[简体中文](AGENT_GUIDE.md) | **English**

> Written specifically for **AI Agents (Claude Code, Codex, Antigravity, Cursor, WorkBuddy, KimiWork, TraeWork, 豆包工作, 千问办公, GrokBot, Muse, Dots)** and **FDEs (Forward Deployed Engineers)** operating AdLink.
>
> **Core Positioning**: Even when individual ad platforms publish their own official MCP servers, **AdLink solves the hard connection problem between fragmented multi-platform raw data and highly customized internal governance (`Person–Account` splits, `Project` classification, `Spend/Performance/Custom` financial calibers, and internal creative labor ROI)** — eliminating custom middleware glue code at every customer site.

---

## Standard Onboarding SOP (For AI Agents)

### Step 1: Probe Runtime Health, WAL Pool & Ad-Creation Policy
1. Call `adlink_health_check` to inspect the microkernel, SQLite WAL reader/writer pool (including `write_epoch`), and all **15 platform connectors** (`10` domestic China connectors + `5` overseas connectors: `tiktok_ads`, `meta_ads`, `google_ads`, `apple_search_ads`, `amazon_ads`) in `mock` or `live` mode.
2. Call `adlink_creation_status_and_risks` to inspect `[ad_creation]` policy (**always `enabled = false` by default**) and retrieve the mandatory user prompt plus 5 risk disclosures.

### Step 2: Two-Tier Login & Account Discovery (Official MCP Bridge First + Direct API Fallback)
For every platform `P` the user wants to connect, call `adlink_onboard_guide(platform: P)` and `adlink_discover_platform_accounts(platform: P)`:

1. **Optimal Path (Tier 1 — Reuse Official Platform MCP Auth + Re-Label Governance)**:
   - If the target media vendor already provides an **Official MCP Server** handling OAuth / account authorization, configure `auth_strategy = "official_mcp_bridge"` and `official_mcp_endpoint` under `[connectors.<platform>]` instead of rebuilding OAuth pages.
   - Call `adlink_discover_platform_accounts` to fetch authorized accounts alongside existing AdLink mappings.
   - Call **`adlink_relabel_account_governance`** to freely **re-label (`重标`)** `PersonAccountBinding` (optimizers, `split_weight`, effective dates) and `ProjectAccountLink` (`project_type`, `campaign_filter_regex`) inside AdLink.

2. **Fallback Path (Tier 2 — Direct Marketing API Sub-Account Pull + Custom Project Bootstrap)**:
   - If no Official MCP exists or headless server auth is needed, instruct the user to place secrets in `.env` and inject `${ENV_VAR}` placeholders into `config/adlink.local.toml`:
     - **Overseas credentials**: `google_ads` requires `developer_token` (plus optional MCC `login_customer_id`); `apple_search_ads` requires `app_id`/`team_id`/`key_id` + `named_curve` ECDSA P-256 PKCS#8 `es256_private_key_pem` + `org_id`; `amazon_ads` requires `app_id` (`Amazon-Ads-ClientId`) + `amazon_profile_id`.
     - **Multi-currency FX normalization**: Configure `extra.currency` and `[fx_rates]` so `USD`/`EUR`/`GBP`/`JPY` spend and revenue convert accurately into canonical CNY `Fen`.
   - Call `adlink_discover_platform_accounts` to list authorized sub-accounts.
   - Once the user selects accounts, call **`adlink_fallback_bootstrap_project`** to create the custom project and link sub-accounts + owners in one step.

3. **Compliance Note (`xhs_pugongying` & `bilibili_huahuo`)**:
   - Neither platform provides an unapproved public self-serve MAPI (and scraping / anti-bot bypass is strictly forbidden).
   - Configure `base_url_override` for whitelisted partner gateways, or import exported order sheets via `adlink_ingest_external_records` (`POST /api/v1/ingest`, optionally with `Idempotency-Key`), preserving `note_id` / `bvid` for closed-loop seeding + boost attribution.

---

### Step 3: Mandatory Default-Off Ad Creation User Prompt & Guardrails

> **MANDATORY RULE**: Always operate in read-only sync + `dry_run` mutation mode unless explicitly authorized. NEVER suggest or enable Ad Creation without first asking the user and disclosing all risks:

1. **Always ask the user first**:
   > "Do you want to opt into programmatic Ad Creation (`创编`: creating new campaigns, ad groups, or creatives via API)?"
2. **Disclose all 5 risks before enabling**:
   - **Real Budget Burn & Unit/Currency Mismatches**: Kuaishou creation uses `li` (`1 Yuan = 1000 li`), Tencent/XHS use `fen`, Ocean Engine/TikTok use major currency units, Meta Insights reports major currency while `/campaigns` & `/adsets` require integer **cents**, and Google uses **micros** (`1e-6`).
   - **Ad Policy Rejection & Account Suspension**: Unreviewed AI-generated copy/creatives risk compliance rejection or account bans.
   - **Targeting, Landing URL & Pixel Misconfiguration**: Wrong geo/audience/conversion IDs cause non-recoverable wasted spend.
   - **Platform Structure & Whitelist Differences**: Tencent AMS v3.0 uses a 2-tier model (`adgroups` + `dynamic_creatives`); XHS/Bilibili require partner whitelists; Alipay Denghuo, XHS Pugongying, and Bilibili Huahuo have no public self-serve creation APIs.
   - **Enforced Post-Enable Guardrails**: User must explicitly set `[ad_creation] enabled = true, risk_acknowledged = true` in `config/adlink.toml`; every `adlink_ad_create` call still forces `initial_status = "PAUSED"`, requires a `dry_run: true` preview before `preview_confirmed: true`, checks budget/bid caps, and writes immutable audit logs.

---

### Step 4: Smoke Verification, Async Queueing & Caliber Reconciliation
1. Run `adlink_sync_reports` over a trailing **3-day** smoke window to verify field mapping and FX conversion. For large historical backfills in production servers, use `POST /api/v1/sync/jobs/enqueue` (`202 Accepted`) and poll `GET /api/v1/sync/jobs/{id}` (monitor probes via `/healthz`, `/readyz`, `/metrics`).
2. Backfill long windows (90–180 days) and call `adlink_reconcile_calibers` to align Gross Media Spend, Cash Spend, Net After Rebate, Full-Funnel Cost, Agency Net Revenue, and Custom AST calibers.
3. Always preview `adlink_ad_mutate` with `dry_run: true` and an explicit `reason` before user-confirmed `dry_run: false`.
