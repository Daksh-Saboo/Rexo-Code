use std::process::Command;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::security::policies::RiskLevel;

use super::{Tool, ToolContext, ToolPermission};

const READ_ONLY: &[&str] = &["status", "diff", "log", "branch", "show", "remote"];
const WRITE: &[&str] = &["add", "commit", "checkout", "push", "pull", "reset", "merge", "tag"];

pub struct GitTool;

#[async_trait]
impl Tool for GitTool {
    fn name(&self) -> &str {
        "git"
    }

    fn description(&self) -> &str {
        "Run a git subcommand in the workspace. Read-only subcommands (status, diff, log, \
         branch, show, remote) run automatically. State-changing subcommands (add, commit, \
         checkout, push, pull, reset, merge, tag) require approval first."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "subcommand": {
                    "type": "string",
                    "description": "The git subcommand, e.g. 'status', 'diff', 'commit'."
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Additional arguments, e.g. [\"-m\", \"fix bug\"] for commit."
                }
            },
            "required": ["subcommand"]
        })
    }

    fn required_permission(&self, _ctx: &ToolContext, arguments: &Value) -> ToolPermission {
        let subcommand = arguments
            .get("subcommand")
            .and_then(Value::as_str)
            .unwrap_or("");

        if READ_ONLY.contains(&subcommand) {
            ToolPermission::Automatic
        } else if WRITE.contains(&subcommand) {
            let args = format_args(arguments);
            ToolPermission::Ask {
                summary: format!("git {subcommand} {args}").trim().to_string(),
                risk: if subcommand == "push" || subcommand == "reset" {
                    RiskLevel::High
                } else {
                    RiskLevel::Moderate
                },
            }
        } else {
            ToolPermission::Denied {
                reason: format!("git subcommand '{subcommand}' is not on the allowed list."),
            }
        }
    }

    async fn execute(&self, ctx: &ToolContext, arguments: Value) -> Result<String> {
        let subcommand = arguments
            .get("subcommand")
            .and_then(Value::as_str)
            .context("Missing required argument 'subcommand'")?;

        if !READ_ONLY.contains(&subcommand) && !WRITE.contains(&subcommand) {
            return Err(anyhow!("git subcommand '{subcommand}' is not allowed"));
        }

        let extra_args: Vec<String> = arguments
            .get("args")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        let workspace = ctx.workspace.clone();
        let subcommand = subcommand.to_string();

        let output = tokio::task::spawn_blocking(move || {
            let mut args = vec![subcommand];
            args.extend(extra_args);
            Command::new("git").args(&args).current_dir(&workspace).output()
        })
        .await
        .context("git command task panicked")?
        .context("Failed to spawn git (is it installed and on PATH?)")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let mut result = String::new();
        result.push_str(&stdout);
        if !stderr.is_empty() {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(&stderr);
        }
        if result.trim().is_empty() {
            result = "(no output)".to_string();
        }
        Ok(result)
    }
}

fn format_args(arguments: &Value) -> String {
    arguments
        .get("args")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn status_is_automatic() {
        let ctx = ToolContext {
            workspace: std::env::temp_dir(),
        extra_read_roots: Vec::new(),
        };
        let tool = GitTool;
        let perm = tool.required_permission(&ctx, &json!({"subcommand": "status"}));
        assert!(matches!(perm, ToolPermission::Automatic));
    }

    #[test]
    fn commit_requires_approval() {
        let ctx = ToolContext {
            workspace: std::env::temp_dir(),
        extra_read_roots: Vec::new(),
        };
        let tool = GitTool;
        let perm = tool.required_permission(&ctx, &json!({"subcommand": "commit", "args": ["-m", "x"]}));
        assert!(matches!(perm, ToolPermission::Ask { .. }));
    }

    #[test]
    fn unknown_subcommand_is_denied() {
        let ctx = ToolContext {
            workspace: std::env::temp_dir(),
        extra_read_roots: Vec::new(),
        };
        let tool = GitTool;
        let perm = tool.required_permission(&ctx, &json!({"subcommand": "filter-branch"}));
        assert!(matches!(perm, ToolPermission::Denied { .. }));
    }
}
