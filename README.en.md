# AdLink — Enterprise Ad-Data & AI-Native MCP Middleware in Rust

[中文版 README](README.md) · [Agent Onboarding Playbook](AGENT_GUIDE.md) · [Architecture](docs/architecture.md) · [Plugin SDK](docs/plugin-development.md)

**AdLink** (Apache-2.0) is a high-performance, DeepSeek-Harness (`Cordis`)-inspired Rust middleware that unifies **8 major Chinese advertising & creator-marketing platforms (10 logical connectors)** with enterprise ad-data governance, financial caliber reconciliation, creative production workload attribution, and a universal multi-transport **Model Context Protocol (MCP)** server.

## Why AdLink?
- **AI-Native Foundation for Advertisers & Agencies**: Raw platform API metrics are unusable in enterprise production without resolving **Person–Account (`人-户关系`)**, **Project Types (`项目类型分类`)**, **Creative Taxonomies & Creator Labor Attribution (`创意分类与内部工时归因`)**, and **Spend / Performance / Custom Calibers (`消耗/业绩/自定义口径`)**.
- **Zero-Friction Deployment**: Ships with an official-schema deterministic mock agency dataset so FDEs, brand growth leads, and agents can test every analytical view immediately, then smoothly onboard live credentials via built-in agent onboarding guides (`adlink_onboard_guide`).
- **Universal MCP Compatibility**: Speaks **Stdio**, **Streamable HTTP** (`/mcp`, both stateful `Mcp-Session-Id` & stateless 2026 clients), and **Legacy SSE** (`/sse` + `/messages`) — ready for **WorkBuddy, KimiWork, TraeWork, 豆包工作, 千问办公, Claude Code, Codex, Antigravity, Cursor, GrokBot, Muse, Dots**, and custom enterprise agents.
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
