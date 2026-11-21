# AdLink — Enterprise Ad-Data & AI-Native MCP Middleware in Rust

[中文版 README](README.md) · [Agent Onboarding Playbook](AGENT_GUIDE.md) · [Field-by-Field Official API Audit](docs/platform-api-mapping.md) · [Architecture](docs/architecture.md) · [Plugin SDK](docs/plugin-development.md)

**AdLink** (Apache-2.0) is a high-performance, DeepSeek-Harness (`Cordis`)-inspired Rust middleware that unifies **8 major Chinese advertising & creator-marketing platforms (10 logical connectors)** with enterprise ad-data governance, financial caliber reconciliation, creative production workload attribution, and a universal multi-transport **Model Context Protocol (MCP)** server (24 tools).

## Why AdLink When Ad Platforms Already Offer Official MCPs?
- **Bridging Raw Media Data & Highly Customized Enterprise Governance**: Official platform MCPs only expose isolated single-platform raw endpoints. **AdLink solves the hard connection problem between fragmented media data and your customized internal Person–Account (`人-户关系`), Project Classification (`项目归属`), Spend/Performance/Custom Calibers (`财务口径`), and Creative Production Labor Attribution (`创意工时归因`)**, eliminating repetitive bespoke glue code and drastically cutting engineering & maintenance costs.
- **Two-Tier Login Architecture**:
  - **Optimal (Tier 1 — Official MCP Auth Bridge + Governance Re-Labeling)**: Directly reuse a platform's official MCP authentication endpoint (`auth_strategy = "official_mcp_bridge"`), discover authorized accounts via `adlink_discover_platform_accounts`, and freely **re-label (`重标`)** `人-户关系` & `项目关系` inside AdLink (`adlink_relabel_account_governance`).
  - **Fallback (Tier 2 — Direct API Account Discovery + Project Bootstrap)**: Pull sub-account lists directly via Marketing API and create custom projects bound to selected sub-accounts (`adlink_fallback_bootstrap_project`).
- **Default-Safe Opt-In Ad Creation (`创编`)**: Programmatic ad creation (`adlink_ad_create` / `POST /api/v1/ad-creation/create`) is **disabled by default (`enabled = false`, `risk_acknowledged = false`)**. Agents MUST ask the user whether they want to opt in and explicitly warn about budget burn, unit mismatches (Kuaishou `li` vs Fen vs Yuan), policy rejection/suspension, targeting mistakes, and API availability limits (e.g., Tencent AMS v3.0 2-tier model; Alipay Denghuo / Pugongying / Huahuo have no public self-serve creation API).
- **Universal MCP Compatibility**: Speaks **Stdio**, **Streamable HTTP** (`/mcp`, stateful & stateless), and **Legacy SSE** (`/sse` + `/messages`) — ready for **WorkBuddy, KimiWork, TraeWork, 豆包工作, 千问办公, Claude Code, Codex, Antigravity, Cursor, GrokBot, Muse, Dots**, and custom enterprise agents.
- **Pluggable Microkernel**: Inspired by DeepSeek Harness / Cordis — connectors, SQLite WAL storage, safety guardrails, calibers, and MCP packs mount via scoped `PluginContext`, interact through typed DI and priority hooks, and cleanly unwind via `EffectScope`.

## Supported Platforms (10 Logical Connectors)

| Platform | Connector ID | API Mode | Max Window | Official Units -> Canonical |
| :--- | :--- | :--- | :--- | :--- |
| Tencent Ads (AMS v3.0) | `tencent_ads` | Public MAPI v3.0 | 90 days | Fen -> `Fen` |
| Xiaohongshu Juguang | `xhs_juguang` | Partner OpenAPI | 30 days | Fen -> `Fen` |
| Xiaohongshu Chengfeng | `xhs_chengfeng` | Partner OpenAPI | 30 days | Fen -> `Fen` |
| Xiaohongshu Pugongying | `xhs_pugongying` | Partner API / Structured Ingest | 30 days | Fen -> `Fen` (`note_id` join) |
| Bilibili Sanlian / Qifei | `bilibili_sanlian` | Commercial Partner MAPI | 30 days | Fen -> `Fen` |
| Bilibili Huahuo | `bilibili_huahuo` | Partner API / Structured Ingest | 30 days | Fen -> `Fen` (`bvid` join) |
| Kuaishou Magnetic Engine | `kuaishou` | Public MAPI v1 | 7 days | Yuan -> `Fen` |
| Ocean Engine AD v3.0 | `ocean_engine_ad` | Public MAPI v3.0 | 30 days | Yuan -> `Fen` |
| Ocean Engine Local Ads v3.0 | `ocean_engine_local` | Public MAPI v3.0 | 30 days | Yuan -> `Fen` |
| Alipay Denghuo | `alipay_denghuo` | Public OpenAPI (**Pure-Rust RSA2**) | 7 days | Fen -> `Fen` |

## Quickstart

```bash
git clone https://github.com/zyfjacksonchen-source/adlink.git
cd adlink
cargo build --release -p adlink-cli
./target/release/adlink serve --config config/adlink.mock.toml --seed-mock
```

Open `http://127.0.0.1:8787/` for the customizable Web Console, or connect your agent to `http://127.0.0.1:8787/mcp` (or `adlink mcp-stdio --config config/adlink.mock.toml --seed-mock`).

## License
Licensed under the **Apache License 2.0**. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
