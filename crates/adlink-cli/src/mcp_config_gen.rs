//! Copy-pasteable MCP client configuration generator for domestic & international AI agents.

use serde_json::json;

pub const SUPPORTED_CLIENTS: &[&str] = &[
    "workbuddy",
    "kimiwork",
    "traework",
    "doubao",
    "qianwen",
    "claude-code",
    "codex",
    "antigravity",
    "cursor",
    "grokbot",
    "muse",
    "dots",
    "http",
];

pub fn render_client_config(client: &str, url: &str, config_path: &str) -> Result<String, String> {
    let base = url.trim_end_matches('/');
    let key = client.trim().to_ascii_lowercase();
    match key.as_str() {
        "codex" => Ok(format!(
            "# OpenAI Codex (~/.codex/config.toml)\n\
            [mcp_servers.adlink]\n\
            command = \"adlink\"\n\
            args = [\"mcp-stdio\", \"--config\", \"{config_path}\", \"--seed-mock\"]\n\n\
            # 或者使用远程 Streamable HTTP 模式:\n\
            # [mcp_servers.adlink]\n\
            # url = \"{base}/mcp\"\n"
        )),
        "claude-code" | "cursor" | "antigravity" | "workbuddy" | "traework" | "muse" | "dots" => {
            let doc = json!({
                "mcpServers": {
                    "adlink": {
                        "command": "adlink",
                        "args": ["mcp-stdio", "--config", config_path, "--seed-mock"],
                        "env": {},
                        "_remote_streamable_http": format!("{base}/mcp"),
                        "_remote_legacy_sse": format!("{base}/sse")
                    }
                }
            });
            serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())
        }
        "kimiwork" | "doubao" | "qianwen" | "grokbot" | "http" => {
            let doc = json!({
                "mcpServers": {
                    "adlink": {
                        "transport": "streamable-http",
                        "url": format!("{base}/mcp"),
                        "legacySseUrl": format!("{base}/sse"),
                        "headers": {
                            "Authorization": "Bearer ${ADLINK_API_KEY}"
                        }
                    }
                }
            });
            serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())
        }
        other => Err(format!(
            "unsupported client '{other}'. Supported: {}",
            SUPPORTED_CLIENTS.join(", ")
        )),
    }
}
