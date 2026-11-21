//! Line-delimited JSON-RPC stdio transport for CLI & IDE agents (Claude Code, Codex, Cursor, Antigravity, WorkBuddy, KimiWork, TraeWork).

use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::McpHandler;

/// Runs the MCP server over stdin/stdout until EOF.
///
/// Tracing logs MUST remain strictly on stderr so stdout carries pure JSON-RPC lines.
pub async fn run_stdio_server(handler: Arc<McpHandler>) -> Result<(), std::io::Error> {
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();

    while let Some(line) = reader.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(resp) = handler.handle_raw_json(trimmed, None).await {
            let encoded = serde_json::to_string(&resp)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            stdout.write_all(encoded.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}
