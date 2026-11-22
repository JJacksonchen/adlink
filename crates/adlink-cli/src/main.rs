//! `adlink` unified CLI entrypoint.

mod bootstrap;
mod config;
mod mcp_config_gen;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use adlink_api::{build_production_app, ApiAuthConfig, ProductionRuntime};
use adlink_domain::{PlatformId, ReportGranularity, ReportLevel, TimeLine};
use adlink_engine::{AdPlatformConnector, SyncCoordinator, SyncRequest};
use adlink_mcp::{get_ad_creation_disclosure, run_stdio_server, McpHandler};
use adlink_storage::StorageService;
use chrono::NaiveDate;
use clap::{Parser, Subcommand};

use crate::bootstrap::{bootstrap_kernel, kernel_plugin_summary, seed_mock_all};
use crate::config::AdLinkConfig;
use crate::mcp_config_gen::{render_client_config, SUPPORTED_CLIENTS};

#[derive(Parser)]
#[command(
    name = "adlink",
    version,
    about = "AdLink — Enterprise Rust Ad-Data & AI-Native MCP Middleware (Apache-2.0)"
)]
struct Cli {
    /// Path to TOML configuration file (optional; defaults to built-in Mock configuration)
    #[arg(long, global = true, env = "ADLINK_CONFIG")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start unified HTTP server (REST API + Streamable HTTP MCP + Legacy SSE MCP + customizable Web GUI)
    Serve {
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        web_dir: Option<PathBuf>,
        /// Seed realistic governance + trailing mock metrics before serving
        #[arg(long)]
        seed_mock: bool,
        /// Days to seed when --seed-mock is enabled
        #[arg(long, default_value_t = 30)]
        mock_days: i64,
        /// Optional comma-separated MCP toolsets filter (onboarding,sync,analytics,governance,ops,system)
        #[arg(long)]
        toolsets: Option<String>,
    },
    /// Start MCP server over Stdio (for Claude Code, Codex, Cursor, Antigravity, WorkBuddy, TraeWork)
    McpStdio {
        #[arg(long)]
        seed_mock: bool,
        #[arg(long, default_value_t = 30)]
        mock_days: i64,
        #[arg(long)]
        toolsets: Option<String>,
    },
    /// Run high-concurrency batch sync across platforms and long date ranges
    Sync {
        #[arg(long)]
        start: NaiveDate,
        #[arg(long)]
        end: NaiveDate,
        /// Comma-separated platforms (defaults to all 15)
        #[arg(long)]
        platforms: Option<String>,
        #[arg(long, default_value_t = 8)]
        concurrency: usize,
        #[arg(long, default_value_t = false)]
        hourly: bool,
        #[arg(long)]
        resume_job_id: Option<String>,
    },
    /// Seed deterministic multi-platform agency governance and trailing mock reports
    SeedMock {
        #[arg(long, default_value_t = 90)]
        days: i64,
        #[arg(long, default_value_t = 12)]
        concurrency: usize,
    },
    /// Print official API onboarding steps for a platform (or all platforms)
    Onboard {
        #[arg(long)]
        platform: Option<String>,
    },
    /// Generate ready-to-paste MCP client configuration for domestic & international AI agents
    McpConfig {
        #[arg(long)]
        client: String,
        #[arg(long, default_value = "http://127.0.0.1:8787")]
        url: String,
    },
    /// Run diagnostic checks on config, SQLite WAL storage, microkernel plugins, and connectors
    Doctor,
    /// List mounted Cordis-inspired microkernel plugins, services, and hook handlers
    Plugins,
}

fn parse_toolsets(raw: Option<String>) -> Option<HashSet<String>> {
    raw.map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(str::to_string)
            .collect()
    })
}

async fn shutdown_signal(prod_runtime: Arc<ProductionRuntime>) {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received: marking /readyz not-ready and draining connections");
    prod_runtime.begin_drain();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cfg = AdLinkConfig::load_or_default(cli.config.as_deref())?;

    // CRITICAL: In `mcp-stdio` mode, stdout carries JSON-RPC frames exclusively;
    // force all tracing logs (both text and json) unconditionally to stderr.
    let log_filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into());
    if cfg.server.production.log_format.eq_ignore_ascii_case("json") {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(log_filter)
            .with_writer(std::io::stderr)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(log_filter)
            .with_writer(std::io::stderr)
            .init();
    }

    match cli.command {
        Commands::Serve {
            host,
            port,
            web_dir,
            seed_mock,
            mock_days,
            toolsets,
        } => {
            let runtime = bootstrap_kernel(&cfg).await?;
            if seed_mock || runtime.storage.count_report_rows().await? == 0 {
                let existing_accounts = runtime.storage.list_ad_accounts(None).await?;
                if seed_mock || existing_accounts.is_empty() {
                    seed_mock_all(&runtime, mock_days, 12).await?;
                }
            }

            let bind_host = host.unwrap_or_else(|| cfg.server.host.clone());
            let bind_port = port.unwrap_or(cfg.server.port);
            let web_root = web_dir.unwrap_or_else(|| PathBuf::from(&cfg.server.web_dir));
            let auth = ApiAuthConfig::from_keys(cfg.auth.api_keys.clone());
            let mcp = Arc::new(McpHandler::new(
                Arc::clone(&runtime.kernel),
                parse_toolsets(toolsets),
            ));
            let (router, prod_runtime) = build_production_app(
                Arc::clone(&runtime.kernel),
                mcp,
                auth,
                Some(web_root.clone()),
                cfg.server.production.clone(),
            );

            let addr = format!("{bind_host}:{bind_port}");
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            tracing::info!(
                listen = %addr,
                web_dir = %web_root.display(),
                reader_pool_size = cfg.storage.reader_pool_size,
                max_in_flight = cfg.server.production.max_in_flight_requests,
                rate_limit_rps = cfg.server.production.rate_limit_rps,
                fx_currencies = runtime.fx_rates.rates_to_cny.len(),
                "AdLink commercial server ready (Probes /healthz /readyz /metrics | REST /api/v1/* | Streamable HTTP /mcp | Legacy SSE /sse)"
            );
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown_signal(Arc::clone(&prod_runtime)))
                .await?;

            let drain_secs = cfg.server.production.shutdown_drain_secs.max(1);
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(drain_secs),
                prod_runtime.shutdown(),
            )
            .await;
            if let Err(e) = runtime.storage.checkpoint_truncate().await {
                tracing::warn!(error = %e, "SQLite WAL checkpoint truncate on shutdown failed");
            } else {
                tracing::info!("AdLink workers drained and SQLite WAL checkpoint truncated");
            }
        }
        Commands::McpStdio {
            seed_mock,
            mock_days,
            toolsets,
        } => {
            let runtime = bootstrap_kernel(&cfg).await?;
            if seed_mock {
                seed_mock_all(&runtime, mock_days, 12).await?;
            }
            let handler = Arc::new(McpHandler::new(
                Arc::clone(&runtime.kernel),
                parse_toolsets(toolsets),
            ));
            run_stdio_server(handler).await?;
        }
        Commands::Sync {
            start,
            end,
            platforms,
            concurrency,
            hourly,
            resume_job_id,
        } => {
            let runtime = bootstrap_kernel(&cfg).await?;
            if runtime.storage.list_ad_accounts(None).await?.is_empty() {
                adlink_connectors::seed_mock_governance(runtime.storage.as_ref()).await?;
            }
            let platform_list: Vec<PlatformId> = match platforms {
                Some(raw) => raw
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.parse::<PlatformId>())
                    .collect::<Result<Vec<_>, _>>()?,
                None => vec![],
            };
            let coord = SyncCoordinator::new(Arc::clone(&runtime.kernel));
            let summary = coord
                .sync_reports(SyncRequest {
                    platforms: platform_list,
                    ad_account_ids: vec![],
                    start_date: start,
                    end_date: end,
                    granularity: if hourly {
                        ReportGranularity::Hourly
                    } else {
                        ReportGranularity::Daily
                    },
                    levels: vec![
                        ReportLevel::Account,
                        ReportLevel::Campaign,
                        ReportLevel::Creative,
                        ReportLevel::ContentOrder,
                    ],
                    time_line: TimeLine::ReportingTime,
                    max_concurrency: concurrency,
                    page_size: 200,
                    resume_job_id,
                    qps_override: None,
                })
                .await?;
            println!("{}", serde_json::to_string_pretty(&summary)?);
        }
        Commands::SeedMock { days, concurrency } => {
            let runtime = bootstrap_kernel(&cfg).await?;
            let total = seed_mock_all(&runtime, days, concurrency).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "seeded": true,
                    "days": days,
                    "total_report_rows": total,
                    "db_path": cfg.storage.db_path
                }))?
            );
        }
        Commands::Onboard { platform } => {
            let runtime = bootstrap_kernel(&cfg).await?;
            let targets: Vec<PlatformId> = match platform {
                Some(p) => vec![p.parse()?],
                None => PlatformId::ALL.to_vec(),
            };
            let guides: Vec<_> = targets
                .into_iter()
                .filter_map(|p| {
                    runtime
                        .kernel
                        .get_named_service::<dyn AdPlatformConnector>(p.as_str())
                        .map(|c| c.onboard_guide())
                })
                .collect();
            let ad_creation_policy = get_ad_creation_disclosure(&runtime.kernel);
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "ad_creation_policy": ad_creation_policy,
                    "guides": guides
                }))?
            );
        }
        Commands::McpConfig { client, url } => {
            let cfg_path = cli
                .config
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "config/adlink.mock.toml".into());
            match render_client_config(&client, &url, &cfg_path) {
                Ok(out) => println!("{out}"),
                Err(err) => {
                    eprintln!("{err}\nAvailable clients: {}", SUPPORTED_CLIENTS.join(", "));
                    std::process::exit(2);
                }
            }
        }
        Commands::Doctor => {
            let runtime = bootstrap_kernel(&cfg).await?;
            let report = runtime.kernel.health_report().await;
            let db = runtime.storage.db_health_summary().await?;
            let connectors: Vec<_> = PlatformId::ALL
                .iter()
                .filter_map(|p| {
                    runtime
                        .kernel
                        .get_named_service::<dyn AdPlatformConnector>(p.as_str())
                        .map(|c| {
                            serde_json::json!({
                                "platform": p.as_str(),
                                "name_zh": p.display_name_zh(),
                                "mode": c.mode_label(),
                                "access_mode": p.meta().access_mode
                            })
                        })
                })
                .collect();
            let ad_creation = get_ad_creation_disclosure(&runtime.kernel);
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "healthy": report.healthy,
                    "db_path": cfg.storage.db_path,
                    "storage": db,
                    "plugins_mounted": report.plugin_count,
                    "fx_rates_to_cny": runtime.fx_rates.rates_to_cny,
                    "ad_creation": ad_creation,
                    "connectors": connectors
                }))?
            );
        }
        Commands::Plugins => {
            let runtime = bootstrap_kernel(&cfg).await?;
            let plugins = runtime.kernel.list_plugins().await;
            println!(
                "{}",
                serde_json::to_string_pretty(&kernel_plugin_summary(&plugins))?
            );
        }
    }

    Ok(())
}
