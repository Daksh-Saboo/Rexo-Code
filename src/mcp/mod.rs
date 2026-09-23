//! A real MCP (Model Context Protocol) client, stdio transport only.
//!
//! Talks JSON-RPC 2.0 to a spawned child process over its stdin/stdout —
//! one JSON object per line, matching the MCP spec's stdio transport (no
//! LSP-style `Content-Length` framing). Three calls make up everything
//! `/mcp connect` needs: [`McpClient::connect`] (spawn + `initialize` +
//! `notifications/initialized`), [`McpClient::list_tools`]
//! (`tools/list`), and [`McpClient::call_tool`] (`tools/call`).
//!
//! What this deliberately does not do: the SSE/HTTP transport (the
//! `url` half of [`crate::config::global::McpServerConfig`] stays
//! unimplemented — stdio covers the overwhelming majority of local MCP
//! servers people actually run), resource/prompt endpoints (only the
//! tool-calling half of MCP, which is what REXO's own tool-use loop can
//! actually act on), and reconnect-on-crash (a dead server just leaves
//! its tools erroring until `/mcp connect` is run again by hand).

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{oneshot, Mutex};

const PROTOCOL_VERSION: &str = "2024-11-05";

#[derive(Debug, Clone)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// A live connection to one MCP server. Cheap to clone — every clone
/// shares the same child process, request-id counter, and pending-reply
/// map, which is what lets each discovered tool hold its own
/// `McpClient` handle (see `register_tools_as_rexo_tools`) without
/// juggling a single `&mut` reference between them.
#[derive(Clone)]
pub struct McpClient {
    stdin: Arc<Mutex<ChildStdin>>,
    pending: PendingMap,
    next_id: Arc<AtomicU64>,
    // Held only to keep the child alive as long as any clone of this
    // client exists, and to let `disconnect_mcp_server` kill it directly
    // on demand — never read from for I/O (the reader task spawned in
    // `connect` owns stdout, `stdin` above owns the write half).
    child: Arc<Mutex<Child>>,
    pub server_name: String,
}

impl McpClient {
    /// A clone of the shared handle to the spawned process — used only
    /// by `Agent::disconnect_mcp_server` to kill it directly (see that
    /// method's doc comment for why relying on `kill_on_drop` alone
    /// isn't enough once proxy tools hold their own clone of this
    /// client too).
    pub(crate) fn child_handle(&self) -> Arc<Mutex<Child>> {
        self.child.clone()
    }
    /// Spawn `command` (split on whitespace — quoted arguments with
    /// embedded spaces aren't supported, same limitation as the
    /// existing `/hooks` shell-out) and complete the MCP handshake:
    /// `initialize` (waits for the response) then
    /// `notifications/initialized` (fire-and-forget, no response
    /// expected on that one — the spec calls it a *notification*).
    pub async fn connect(server_name: &str, command: &str) -> Result<Self> {
        let mut parts = command.split_whitespace();
        let program = parts.next().context("empty command")?;
        let args: Vec<&str> = parts.collect();

        let mut child = tokio::process::Command::new(program)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("couldn't start `{command}`"))?;

        let stdin = child.stdin.take().context("no stdin on spawned MCP server")?;
        let stdout = child.stdout.take().context("no stdout on spawned MCP server")?;

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let reader_pending = pending.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let Ok(msg) = serde_json::from_str::<Value>(line) else { continue };
                let Some(id) = msg.get("id").and_then(Value::as_u64) else { continue }; // skip server->client requests/notifications — not used by any server this client talks to
                let mut pending = reader_pending.lock().await;
                if let Some(tx) = pending.remove(&id) {
                    if let Some(err) = msg.get("error") {
                        let reason = err.get("message").and_then(Value::as_str).unwrap_or("unknown MCP error").to_string();
                        let _ = tx.send(Err(reason));
                    } else {
                        let _ = tx.send(Ok(msg.get("result").cloned().unwrap_or(Value::Null)));
                    }
                }
            }
            // Reader loop ended (server exited / closed stdout) — drop
            // every still-pending request rather than let its caller
            // hang forever waiting on a reply that will never arrive.
            let mut pending = reader_pending.lock().await;
            for (_, tx) in pending.drain() {
                let _ = tx.send(Err("MCP server exited before replying".to_string()));
            }
        });

        let client = Self { stdin: Arc::new(Mutex::new(stdin)), pending, next_id: Arc::new(AtomicU64::new(1)), child: Arc::new(Mutex::new(child)), server_name: server_name.to_string() };

        let init_result = client
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "rexo-code", "version": env!("CARGO_PKG_VERSION")}
                }),
            )
            .await
            .with_context(|| format!("`{server_name}` didn't respond to initialize"))?;
        let _ = init_result; // nothing in the response REXO currently needs beyond "it answered"

        client.notify("notifications/initialized", json!({})).await?;
        Ok(client)
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let payload = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let line = format!("{}\n", serde_json::to_string(&payload)?);
        self.stdin.lock().await.write_all(line.as_bytes()).await.with_context(|| format!("couldn't write {method} to {}", self.server_name))?;

        match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(reason))) => Err(anyhow!("{}", reason)),
            Ok(Err(_)) => Err(anyhow!("{} closed the connection before replying", self.server_name)),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(anyhow!("{} did not respond to {method} within 30s", self.server_name))
            }
        }
    }

    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let payload = json!({"jsonrpc": "2.0", "method": method, "params": params});
        let line = format!("{}\n", serde_json::to_string(&payload)?);
        self.stdin.lock().await.write_all(line.as_bytes()).await.with_context(|| format!("couldn't write {method} to {}", self.server_name))?;
        Ok(())
    }

    pub async fn list_tools(&self) -> Result<Vec<McpToolInfo>> {
        let result = self.request("tools/list", json!({})).await?;
        let tools = result.get("tools").and_then(Value::as_array).cloned().unwrap_or_default();
        Ok(tools
            .into_iter()
            .filter_map(|t| {
                let name = t.get("name")?.as_str()?.to_string();
                let description = t.get("description").and_then(Value::as_str).unwrap_or("").to_string();
                let input_schema = t.get("inputSchema").cloned().unwrap_or_else(|| json!({"type": "object", "properties": {}}));
                Some(McpToolInfo { name, description, input_schema })
            })
            .collect())
    }

    /// Calls the tool and flattens MCP's `content` array (a list of
    /// `{type: "text", text: "..."}` parts — non-text part types, e.g.
    /// `image`, are skipped rather than erroring the whole call, since
    /// REXO's tool-result channel is text-only) into one string.
    pub async fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<String> {
        let result = self.request("tools/call", json!({"name": tool_name, "arguments": arguments})).await?;
        let is_error = result.get("isError").and_then(Value::as_bool).unwrap_or(false);
        let content = result.get("content").and_then(Value::as_array).cloned().unwrap_or_default();
        let text = content
            .iter()
            .filter_map(|part| if part.get("type").and_then(Value::as_str) == Some("text") { part.get("text").and_then(Value::as_str) } else { None })
            .collect::<Vec<_>>()
            .join("\n");
        if is_error {
            Err(anyhow!("{}", if text.is_empty() { "tool call failed".to_string() } else { text }))
        } else {
            Ok(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `tests/fixtures/fake_mcp_server.py` — a real, standalone MCP
    /// server (stdio JSON-RPC, no REXO code involved on that side) used
    /// to exercise the actual wire protocol end to end, the same way
    /// `/hooks`'s tests spawn real `sh -c` commands rather than mocking
    /// process execution.
    fn fixture_path() -> String {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        format!("{manifest_dir}/tests/fixtures/fake_mcp_server.py")
    }

    fn python3_available() -> bool {
        std::process::Command::new("python3").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
    }

    #[tokio::test]
    async fn connect_and_list_tools_against_a_real_stdio_server() {
        if !python3_available() {
            eprintln!("skipping: no python3 in this environment");
            return;
        }
        let client = McpClient::connect("fake", &format!("python3 {}", fixture_path())).await.unwrap();
        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
    }

    #[tokio::test]
    async fn call_tool_round_trips_a_real_argument() {
        if !python3_available() {
            eprintln!("skipping: no python3 in this environment");
            return;
        }
        let client = McpClient::connect("fake", &format!("python3 {}", fixture_path())).await.unwrap();
        let result = client.call_tool("echo", json!({"text": "hello from a test"})).await.unwrap();
        assert_eq!(result, "hello from a test");
    }

    #[tokio::test]
    async fn calling_an_unknown_tool_surfaces_the_servers_error() {
        if !python3_available() {
            eprintln!("skipping: no python3 in this environment");
            return;
        }
        let client = McpClient::connect("fake", &format!("python3 {}", fixture_path())).await.unwrap();
        let err = client.call_tool("nope", json!({})).await.unwrap_err();
        assert!(err.to_string().contains("unknown tool"));
    }
}
