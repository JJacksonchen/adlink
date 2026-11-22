# AdLink — Enterprise Ad-Data & AI-Native MCP Middleware in Rust

[中文版 README](README.md) · [Agent Onboarding Playbook](AGENT_GUIDE.md) · [Field-by-Field Official API Audit](docs/platform-api-mapping.md) · [Architecture](docs/architecture.md) · [Plugin SDK](docs/plugin-development.md)

**AdLink** (Apache-2.0) is a high-performance, DeepSeek-Harness (`Cordis`)-inspired Rust middleware that unifies **15 mainstream advertising platforms (10 domestic China connectors + 5 global overseas connectors)** with ISO-4217 multi-currency FX conversion, enterprise ad-data governance, financial caliber reconciliation, creative production workload attribution, a **commercial high-concurrency production server**, and a universal multi-transport **Model Context Protocol (MCP)** server (24 tools).

## Why AdLink When Ad Platforms Already Offer Official MCPs?
- **Bridging Raw Media Data & Highly Customized Enterprise Governance**: Official platform MCPs only expose isolated single-platform raw endpoints. **AdLink solves the hard connection problem between fragmented media data and your customized internal Person–Account (`人-户关系`), Project Classification (`项目归属`), Spend/Performance/Custom Calibers (`财务口径`), and Creative Production Labor Attribution (`创意工时归因`)**, eliminating repetitive bespoke glue code and drastically cutting engineering & maintenance costs.
- **Two-Tier Login Architecture**:
  - **Optimal (Tier 1 — Official MCP Auth Bridge + Governance Re-Labeling)**: Directly reuse a platform's official MCP authentication endpoint (`auth_strategy = "official_mcp_bridge"`), discover authorized accounts via `adlink_discover_platform_accounts`, and freely **re-label (`重标`)** `人-户关系` & `项目关系` inside AdLink (`adlink_relabel_account_governance`).
  - **Fallback (Tier 2 — Direct API Account Discovery + Project Bootstrap)**: Pull sub-account lists directly via Marketing API and create custom projects bound to selected sub-accounts (`adlink_fallback_bootstrap_project`).
- **Default-Safe Opt-In Ad Creation (`创编`)**: Programmatic ad creation (`adlink_ad_create` / `POST /api/v1/ad-creation/create`) is **disabled by default (`enabled = false`, `risk_acknowledged = false`)**. Agents MUST ask the user whether they want to opt in and explicitly warn about budget burn, unit mismatches (Kuaishou `li` vs Fen vs Yuan), policy rejection/suspension, targeting mistakes, and API availability limits (e.g., Tencent AMS v3.0 2-tier model; Alipay Denghuo / Pugongying / Huahuo have no public self-serve creation API).
- **Universal MCP Compatibility**: Speaks **Stdio**, **Streamable HTTP** (`/mcp`, stateful & stateless), and **Legacy SSE** (`/sse` + `/messages`) — ready for **WorkBuddy, KimiWork, TraeWork, 豆包工作, 千问办公, Claude Code, Codex, Antigravity, Cursor, GrokBot, Muse, Dots**, and custom enterprise agents.
- **Pluggable Microkernel**: Inspired by DeepSeek Harness / Cordis — connectors, SQLite WAL storage, safety guardrails, calibers, and MCP packs mount via scoped `PluginContext`, interact through typed DI and priority hooks, and cleanly unwind via `EffectScope`.

## Supported Platforms (15 Connectors = 10 Domestic CN + 5 Global Overseas)

| Platform | Connector ID | Region / Mode | Max Window | Official Units -> Canonical CNY `Fen` |
| :--- | :--- | :--- | :--- | :--- |
| Tencent Ads (AMS v3.0) | `tencent_ads` | CN · Public MAPI v3.0 | 90 days | Fen -> `Fen` |
| Xiaohongshu Juguang | `xhs_juguang` | CN · Partner OpenAPI | 30 days | Fen / Yuan string -> `Fen` |
| Xiaohongshu Chengfeng | `xhs_chengfeng` | CN · Partner OpenAPI | 30 days | Fen / Yuan string -> `Fen` |
| Xiaohongshu Pugongying | `xhs_pugongying` | CN · Partner API / Structured Ingest | 30 days | Fen -> `Fen` (`note_id` join) |
| Bilibili Sanlian / Qifei | `bilibili_sanlian` | CN · Commercial Partner MAPI | 30 days | Fen -> `Fen` |
| Bilibili Huahuo | `bilibili_huahuo` | CN · Partner API / Structured Ingest | 30 days | Fen -> `Fen` (`bvid` join) |
| Kuaishou Magnetic Engine | `kuaishou` | CN · Public MAPI v1 | 7 days | Yuan (report) / Li (create) -> `Fen` |
| Ocean Engine AD v3.0 | `ocean_engine_ad` | CN · Public MAPI v3.0 | 30 days | Yuan -> `Fen` |
| Ocean Engine Local Ads v3.0 | `ocean_engine_local` | CN · Public MAPI v3.0 | 30 days | Yuan -> `Fen` |
| Alipay Denghuo | `alipay_denghuo` | CN · Public OpenAPI (**Pure-Rust RSA2**) | 7 days | Fen -> `Fen` |
| **TikTok Ads** | `tiktok_ads` | Global · Marketing API v1.3 | 30 days | Major currency × FX -> `Fen` |
| **Meta Ads (FB/IG)** | `meta_ads` | Global · Graph API v21.0 | 30 days | Insights major / Create cents × FX (`omni_purchase`) |
| **Google Ads** | `google_ads` | Global · Google Ads API v18 (GAQL) | 90 days | `costMicros` (`1e-6`) × FX -> `Fen` |
| **Apple Search Ads** | `apple_search_ads` | Global · Ads API v5 (**Pure-Rust ES256**) | 30 days | `localSpend` × FX -> `Fen` *(v5 sunset Jan 2027)* |
| **Amazon Ads** | `amazon_ads` | Global · Ads API v3 (Async) | 31 days | `cost` × FX -> `Fen` *(v3 migration Jun 2027)* |

## Commercial High-Concurrency Server Architecture
- **SQLite WAL Reader/Writer Pool**: Serialized transactional writer with monotonic `write_epoch` + configurable read-only pool (`reader_pool_size = 8`, `query_only = ON`, `mmap_size = 256MB`) + shutdown `PRAGMA wal_checkpoint(TRUNCATE)`.
- **Observability & Probes**: `GET /healthz`, `GET /readyz` (switches to `503` on `SIGTERM` drain), `GET /metrics` (Prometheus text exposition), `X-Request-Id`, security headers, structured JSON/text `stderr` logs.
- **Traffic & Reliability Guardrails**: Epoch-invalidated `AnalyticsQueryCache` (`X-Cache: HIT|MISS`), `Idempotency-Key` deduplication (`X-Idempotency-Replayed: true`), per-tenant token-bucket rate limiting (`429`), adaptive concurrency load shedding (`503`), request timeouts (`504`, preserving `/sse`), bounded async sync worker queue (`POST /api/v1/sync/jobs/enqueue` -> `202`, `GET /api/v1/sync/jobs/{id}`), webhook dispatcher, and production Nginx + Kubernetes StatefulSet manifests under `deploy/`.

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
