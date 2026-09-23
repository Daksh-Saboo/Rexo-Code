//! A real local server for editor/IDE integration (`/ide start`).
//!
//! Honest framing up front: this is the REXO-side half of a protocol.
//! There is no companion VSCode/JetBrains extension — building one is a
//! genuinely separate project (a different language ecosystem, its own
//! packaging and publishing pipeline) that doesn't belong in this Rust
//! binary. What's real and tested here is the server itself: it binds a
//! real TCP socket, writes a discoverable lockfile the way Claude Code's
//! own IDE integration does (`~/.rexo/ide/<port>.json`, so a future
//! extension knows where to connect without the user typing a port in
//! by hand), speaks a real newline-delimited JSON protocol, and is
//! verified against a synthetic test client — the same honesty pattern
//! `mcp::McpClient` and `hooks` use elsewhere in this release: real code,
//! tested against something REXO itself controls, not against the
//! external counterpart it's ultimately meant for.
//!
//! Protocol (newline-delimited JSON, request/response by `id`):
//! - `{"id":1,"method":"workspace_info"}` → `{"id":1,"result":{"workspace":"<path>"}}`
//! - `{"id":2,"method":"notify_open_file","params":{"path":"..."}}` → `{"id":2,"result":{"ok":true}}`
//! - `{"id":3,"method":"notify_selection","params":{"path":"...","start_line":N,"end_line":M}}` → `{"id":3,"result":{"ok":true}}`
//!
//! `notify_open_file`/`notify_selection` just update
//! [`IdeState::current_context`] — what a connected editor last reported
//! as open/selected — which `/ide status` (and, in principle, a future
//! prompt-augmentation step) can read. There is currently no
//! REXO-initiated messages to the client (no "open this file",
//! "show this diff") — only the client-to-server half exists yet.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug, Clone, Default)]
pub struct IdeContext {
    pub open_file: Option<String>,
    pub selection: Option<(String, u32, u32)>,
}

struct IdeState {
    workspace: PathBuf,
    context: Mutex<IdeContext>,
}

/// A running server. Dropping this (or calling [`stop`]) closes the
/// listener and removes the lockfile; existing client connections are
/// simply not accepted further, they aren't forcibly killed — not
/// needed for a local dev-loop integration like this.
pub struct IdeServer {
    pub port: u16,
    lockfile: PathBuf,
    state: Arc<IdeState>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl IdeServer {
    pub fn current_context(&self) -> IdeContext {
        self.state.context.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Drop for IdeServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        let _ = std::fs::remove_file(&self.lockfile);
    }
}

fn lockfile_dir() -> Result<PathBuf> {
    Ok(crate::config::global::global_dir()?.join("ide"))
}

/// Bind a local TCP port (OS-assigned — never a fixed, guessable port),
/// write the discovery lockfile, and start accepting connections in the
/// background.
pub async fn start(workspace: &std::path::Path) -> Result<IdeServer> {
    let listener = TcpListener::bind("127.0.0.1:0").await.context("couldn't bind a local port for the IDE server")?;
    let port = listener.local_addr()?.port();

    let dir = lockfile_dir()?;
    std::fs::create_dir_all(&dir)?;
    let lockfile = dir.join(format!("{port}.json"));
    std::fs::write(&lockfile, serde_json::to_string_pretty(&json!({"port": port, "workspace": workspace, "pid": std::process::id()}))?)?;

    let state = Arc::new(IdeState { workspace: workspace.to_path_buf(), context: Mutex::new(IdeContext::default()) });
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();

    let accept_state = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { continue };
                    tokio::spawn(handle_connection(stream, accept_state.clone()));
                }
            }
        }
    });

    Ok(IdeServer { port, lockfile, state, shutdown: Some(shutdown_tx) })
}

pub fn stop(server: IdeServer) {
    drop(server); // Drop impl does the actual work — see above
}

async fn handle_connection(stream: TcpStream, state: Arc<IdeState>) {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(line) else { continue };
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        let result = match method {
            "workspace_info" => json!({"workspace": state.workspace}),
            "notify_open_file" => {
                if let Some(path) = params.get("path").and_then(Value::as_str) {
                    let mut ctx = state.context.lock().unwrap_or_else(|e| e.into_inner());
                    ctx.open_file = Some(path.to_string());
                }
                json!({"ok": true})
            }
            "notify_selection" => {
                if let (Some(path), Some(start), Some(end)) =
                    (params.get("path").and_then(Value::as_str), params.get("start_line").and_then(Value::as_u64), params.get("end_line").and_then(Value::as_u64))
                {
                    let mut ctx = state.context.lock().unwrap_or_else(|e| e.into_inner());
                    ctx.selection = Some((path.to_string(), start as u32, end as u32));
                }
                json!({"ok": true})
            }
            other => {
                let response = json!({"id": id, "error": {"code": -32601, "message": format!("method not found: {other}")}});
                let _ = write_half.write_all(format!("{response}\n").as_bytes()).await;
                continue;
            }
        };
        let response = json!({"id": id, "result": result});
        if write_half.write_all(format!("{response}\n").as_bytes()).await.is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    /// A tiny synthetic client exercising the real wire protocol against
    /// a real running server — the honest substitute for a real IDE
    /// extension, which doesn't exist yet (see module docs).
    async fn send_and_recv(port: u16, request: Value) -> Value {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream.write_all(format!("{request}\n").as_bytes()).await.unwrap();
        let mut buf = vec![0u8; 4096];
        let n = stream.read(&mut buf).await.unwrap();
        serde_json::from_slice(&buf[..n]).unwrap()
    }

    #[tokio::test]
    async fn workspace_info_returns_the_real_workspace() {
        let ws = std::env::temp_dir();
        let server = start(&ws).await.unwrap();
        let resp = send_and_recv(server.port, json!({"id": 1, "method": "workspace_info"})).await;
        assert_eq!(resp["result"]["workspace"], ws.to_string_lossy().to_string());
    }

    #[tokio::test]
    async fn notify_open_file_updates_current_context() {
        let ws = std::env::temp_dir();
        let server = start(&ws).await.unwrap();
        let resp = send_and_recv(server.port, json!({"id": 2, "method": "notify_open_file", "params": {"path": "/repo/src/main.rs"}})).await;
        assert_eq!(resp["result"]["ok"], true);
        assert_eq!(server.current_context().open_file.as_deref(), Some("/repo/src/main.rs"));
    }

    #[tokio::test]
    async fn notify_selection_updates_current_context() {
        let ws = std::env::temp_dir();
        let server = start(&ws).await.unwrap();
        send_and_recv(server.port, json!({"id": 3, "method": "notify_selection", "params": {"path": "/repo/src/lib.rs", "start_line": 10, "end_line": 20}})).await;
        let ctx = server.current_context();
        assert_eq!(ctx.selection, Some(("/repo/src/lib.rs".to_string(), 10, 20)));
    }

    #[tokio::test]
    async fn unknown_method_gets_a_json_rpc_error_not_a_dropped_connection() {
        let ws = std::env::temp_dir();
        let server = start(&ws).await.unwrap();
        let resp = send_and_recv(server.port, json!({"id": 4, "method": "nonexistent"})).await;
        assert!(resp.get("error").is_some());
    }

    #[tokio::test]
    async fn lockfile_is_written_and_removed_on_stop() {
        let ws = std::env::temp_dir();
        let server = start(&ws).await.unwrap();
        let lockfile = lockfile_dir().unwrap().join(format!("{}.json", server.port));
        assert!(lockfile.exists());
        stop(server);
        // Drop runs synchronously — no await needed before checking.
        assert!(!lockfile.exists());
    }
}
