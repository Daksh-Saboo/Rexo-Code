use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::security::policies::{classify_command, CommandRisk, RiskLevel};

use super::{Tool, ToolContext, ToolPermission};

const MAX_OUTPUT_CHARS: usize = 20_000;

/// Run `command` through the OS shell (PowerShell on Windows, `sh`
/// elsewhere) in `workspace`, blocking the calling thread — callers on an
/// async runtime should wrap this in `tokio::task::spawn_blocking`, which
/// both call sites in this crate do (the agent's `run_command` tool
/// below, and user-invoked shell mode in `cli::tui`). Pulled out as its
/// own function so there's exactly one place that knows how to invoke a
/// shell on each platform, rather than two copies drifting apart.
pub fn spawn_os_shell(command: &str, workspace: &Path) -> std::io::Result<Output> {
    if cfg!(target_os = "windows") {
        Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", command]).current_dir(workspace).output()
    } else {
        Command::new("sh").args(["-c", command]).current_dir(workspace).output()
    }
}

pub struct RunCommandTool;

#[async_trait]
impl Tool for RunCommandTool {
    fn name(&self) -> &str {
        "run_command"
    }

    fn description(&self) -> &str {
        "Run a shell command in the workspace directory (PowerShell on Windows, sh elsewhere) \
         and return its combined stdout/stderr. Read-only commands like `cargo check`, \
         `cargo test`, or `git status` run automatically; anything else asks for approval first. \
         Dangerous, destructive commands are always blocked."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to run."
                }
            },
            "required": ["command"]
        })
    }

    fn required_permission(&self, _ctx: &ToolContext, arguments: &Value) -> ToolPermission {
        let command = arguments.get("command").and_then(Value::as_str).unwrap_or("");

        match classify_command(command) {
            CommandRisk::Safe => ToolPermission::Automatic,
            CommandRisk::RequiresApproval => ToolPermission::Ask {
                summary: format!("Run: {command}"),
                risk: RiskLevel::Moderate,
            },
            CommandRisk::Dangerous => ToolPermission::Denied {
                reason: format!(
                    "'{command}' matches REXO's dangerous-command policy and is always blocked, \
                     regardless of approval. Run it yourself outside REXO if you really need to."
                ),
            },
        }
    }

    async fn execute(&self, ctx: &ToolContext, arguments: Value) -> Result<String> {
        let command = arguments
            .get("command")
            .and_then(Value::as_str)
            .context("Missing required argument 'command'")?;

        // Defense in depth: re-check even though the agent loop already
        // consulted required_permission before calling execute. This tool
        // must never run a Dangerous command no matter how it's invoked.
        if classify_command(command) == CommandRisk::Dangerous {
            return Ok(format!(
                "Blocked: '{command}' matches the dangerous-command policy and was not run."
            ));
        }

        let workspace = ctx.workspace.clone();
        let command = command.to_string();

        let output = tokio::task::spawn_blocking(move || spawn_os_shell(&command, &workspace))
            .await
            .context("Command execution task panicked")?
            .context("Failed to spawn command")?;

        Ok(format_command_output(&output, MAX_OUTPUT_CHARS))
    }
}

/// Combine stdout/stderr into one string the way both `run_command` and
/// user-invoked shell mode present it: stdout, then a `--- stderr ---`
/// separator only if there's stderr *and* stdout wasn't empty, then a
/// trailing exit-code note on failure, truncated to `max_chars` for the
/// model's benefit (shell mode itself doesn't truncate — see its caller).
pub fn format_command_output(output: &Output, max_chars: usize) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let mut result = String::new();
    if !stdout.is_empty() {
        result.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !result.is_empty() {
            result.push_str("\n--- stderr ---\n");
        }
        result.push_str(&stderr);
    }
    if !output.status.success() {
        result.push_str(&format!("\n(exit code: {})", output.status.code().unwrap_or(-1)));
    }
    if result.trim().is_empty() {
        result = "(command produced no output)".to_string();
    }

    if result.len() > max_chars {
        result.truncate(max_chars);
        result.push_str("\n... output truncated ...");
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn runs_a_trivial_command() {
        let dir = std::env::temp_dir();
        let ctx = ToolContext {
            workspace: dir,
        extra_read_roots: Vec::new(),
        };
        let tool = RunCommandTool;
        let cmd = if cfg!(target_os = "windows") {
            "echo hello"
        } else {
            "echo hello"
        };
        let result = tool.execute(&ctx, json!({"command": cmd})).await.unwrap();
        assert!(result.to_lowercase().contains("hello"));
    }

    #[tokio::test]
    async fn dangerous_command_is_blocked_even_if_called_directly() {
        let dir = std::env::temp_dir();
        let ctx = ToolContext {
            workspace: dir,
        extra_read_roots: Vec::new(),
        };
        let tool = RunCommandTool;
        let result = tool
            .execute(&ctx, json!({"command": "rm -rf /"}))
            .await
            .unwrap();
        assert!(result.starts_with("Blocked"));
    }

    // ---- spawn_os_shell / format_command_output — shared by both the
    // agent's run_command tool (above) and user-invoked shell mode
    // (cli::tui::run_shell_command) ----

    #[test]
    fn spawn_os_shell_runs_in_the_given_workspace() {
        let dir = std::env::temp_dir();
        let cmd = if cfg!(target_os = "windows") { "echo shell-mode-test" } else { "echo shell-mode-test" };
        let output = spawn_os_shell(cmd, &dir).unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).to_lowercase().contains("shell-mode-test"));
    }

    #[test]
    fn format_command_output_combines_stdout_and_stderr() {
        let dir = std::env::temp_dir();
        let cmd = if cfg!(target_os = "windows") { "echo out; [Console]::Error.WriteLine('err')" } else { "echo out; echo err 1>&2" };
        let output = spawn_os_shell(cmd, &dir).unwrap();
        let text = format_command_output(&output, 20_000);
        assert!(text.to_lowercase().contains("out"));
        assert!(text.to_lowercase().contains("err"));
    }

    #[test]
    fn format_command_output_notes_a_nonzero_exit_code() {
        let dir = std::env::temp_dir();
        let cmd = if cfg!(target_os = "windows") { "exit 3" } else { "exit 3" };
        let output = spawn_os_shell(cmd, &dir).unwrap();
        let text = format_command_output(&output, 20_000);
        assert!(text.contains("exit code: 3"));
    }

    #[test]
    fn format_command_output_truncates_long_output() {
        let output = std::process::Command::new(if cfg!(target_os = "windows") { "powershell" } else { "sh" })
            .args(if cfg!(target_os = "windows") {
                vec!["-NoProfile".to_string(), "-Command".to_string(), "'x' * 200".to_string()]
            } else {
                vec!["-c".to_string(), "printf 'x%.0s' $(seq 1 200)".to_string()]
            })
            .output()
            .unwrap();
        let text = format_command_output(&output, 50);
        assert!(text.len() < 100);
        assert!(text.contains("truncated"));
    }
}
