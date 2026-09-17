use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::security::policies::validate_workspace_path;

use super::filesystem::IGNORED_DIR_NAMES;
use super::{Tool, ToolContext, ToolPermission};

const MAX_MATCHES: usize = 200;
const MAX_FILE_SIZE_BYTES: u64 = 5 * 1024 * 1024;

pub struct SearchFilesTool;

#[async_trait]
impl Tool for SearchFilesTool {
    fn name(&self) -> &str {
        "search_files"
    }

    fn description(&self) -> &str {
        "Recursively search text files under a directory for a literal substring or query. \
         Returns matching file paths with the matching line and line number. \
         Skips .git, target, node_modules, and other build/dependency directories."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "root": {
                    "type": "string",
                    "description": "Directory to search from, relative to the workspace root. Defaults to '.'."
                },
                "query": {
                    "type": "string",
                    "description": "Literal substring to search for (case-sensitive)."
                }
            },
            "required": ["query"]
        })
    }

    fn required_permission(&self, _ctx: &ToolContext, _arguments: &Value) -> ToolPermission {
        ToolPermission::Automatic
    }

    async fn execute(&self, ctx: &ToolContext, arguments: Value) -> Result<String> {
        let root = arguments.get("root").and_then(Value::as_str).unwrap_or(".");
        let query = arguments
            .get("query")
            .and_then(Value::as_str)
            .context("Missing required argument 'query'")?;

        if query.is_empty() {
            return Ok("Query must not be empty.".to_string());
        }

        let resolved_root = validate_workspace_path(&ctx.workspace, root)?;

        let mut matches = Vec::new();
        let mut truncated = false;
        search_directory(&resolved_root, &ctx.workspace, query, &mut matches, &mut truncated)?;

        if matches.is_empty() {
            return Ok(format!("No matches for \"{query}\" under {root}"));
        }

        let mut out = matches.join("\n");
        if truncated {
            out.push_str(&format!(
                "\n... results truncated at {MAX_MATCHES} matches; narrow your query"
            ));
        }
        Ok(out)
    }
}

fn search_directory(
    dir: &Path,
    workspace: &Path,
    query: &str,
    matches: &mut Vec<String>,
    truncated: &mut bool,
) -> Result<()> {
    if *truncated || !dir.is_dir() {
        return Ok(());
    }

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()), // permission-denied etc: skip silently
    };

    for entry in entries {
        if *truncated {
            return Ok(());
        }
        let entry = entry?;
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();

        if path.is_dir() {
            if IGNORED_DIR_NAMES.contains(&name) {
                continue;
            }
            search_directory(&path, workspace, query, matches, truncated)?;
        } else if path.is_file() {
            if let Ok(meta) = fs::metadata(&path) {
                if meta.len() > MAX_FILE_SIZE_BYTES {
                    continue;
                }
            }
            if let Ok(content) = fs::read_to_string(&path) {
                let rel = path
                    .strip_prefix(workspace)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");

                for (i, line) in content.lines().enumerate() {
                    if line.contains(query) {
                        matches.push(format!("{rel}:{}: {}", i + 1, line.trim()));
                        if matches.len() >= MAX_MATCHES {
                            *truncated = true;
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn finds_matching_line_with_location() {
        let dir = std::env::temp_dir().join("rexo_test_search");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.rs"), "fn main() {\n    // authentication here\n}").unwrap();

        let ctx = ToolContext {
            workspace: dir.clone(),
        extra_read_roots: Vec::new(),
        };
        let tool = SearchFilesTool;
        let result = tool
            .execute(&ctx, json!({"query": "authentication"}))
            .await
            .unwrap();
        assert!(result.contains("a.rs:2"));
    }
}
