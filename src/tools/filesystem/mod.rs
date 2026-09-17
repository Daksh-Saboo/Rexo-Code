use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::security::policies::validate_read_path;

use super::{Tool, ToolContext, ToolPermission};

/// Directories we never want to walk into when listing/searching, mirroring
/// the exclusions already used by the search tool.
pub const IGNORED_DIR_NAMES: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".next",
    "dist",
    "build",
    ".venv",
    "__pycache__",
];

pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the full contents of a UTF-8 text file inside the project workspace \
         (or, given an absolute path, inside a directory the user added with /add-dir). \
         Use this before editing a file so your patch is based on its real, current content."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file, relative to the workspace root — or an absolute path, either inside the workspace or inside a directory added with /add-dir."
                }
            },
            "required": ["path"]
        })
    }

    // Read-only, no side effects: always automatic.
    fn required_permission(&self, _ctx: &ToolContext, _arguments: &Value) -> ToolPermission {
        ToolPermission::Automatic
    }

    async fn execute(&self, ctx: &ToolContext, arguments: Value) -> Result<String> {
        let path = arguments
            .get("path")
            .and_then(Value::as_str)
            .context("Missing required argument 'path'")?;

        let resolved = validate_read_path(&ctx.workspace, &ctx.extra_read_roots, path)?;

        if !resolved.exists() {
            return Ok(format!("File not found: {path}"));
        }
        if resolved.is_dir() {
            return Ok(format!(
                "'{path}' is a directory, not a file. Use list_files instead."
            ));
        }

        let bytes = fs::read(&resolved).with_context(|| format!("Failed to read file: {path}"))?;
        match String::from_utf8(bytes) {
            Ok(text) => {
                let numbered = number_lines(&text);
                Ok(numbered)
            }
            Err(_) => Ok(format!(
                "'{path}' is not valid UTF-8 text (likely a binary file); refusing to read it as text."
            )),
        }
    }
}

/// Prefix every line with its 1-based line number, which makes it much
/// easier for the model to produce accurate, targeted edits and to discuss
/// specific lines with the user.
fn number_lines(text: &str) -> String {
    let width = text.lines().count().to_string().len().max(4);
    text.lines()
        .enumerate()
        .map(|(i, line)| format!("{:>width$} | {}", i + 1, line, width = width))
        .collect::<Vec<_>>()
        .join("\n")
}

pub struct ListFilesTool;

#[async_trait]
impl Tool for ListFilesTool {
    fn name(&self) -> &str {
        "list_files"
    }

    fn description(&self) -> &str {
        "List files and directories under a path inside the workspace (or, given an \
         absolute path, inside a directory the user added with /add-dir). \
         Set recursive=true to walk the whole subtree (skips .git, target, \
         node_modules, and other build/dependency directories automatically)."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory to list, relative to the workspace root. Defaults to '.'."
                },
                "recursive": {
                    "type": "boolean",
                    "description": "If true, recursively list the whole subtree. Defaults to false."
                }
            }
        })
    }

    fn required_permission(&self, _ctx: &ToolContext, _arguments: &Value) -> ToolPermission {
        ToolPermission::Automatic
    }

    async fn execute(&self, ctx: &ToolContext, arguments: Value) -> Result<String> {
        let path = arguments
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or(".");
        let recursive = arguments
            .get("recursive")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let resolved = validate_read_path(&ctx.workspace, &ctx.extra_read_roots, path)?;

        if !resolved.exists() {
            return Ok(format!("Directory not found: {path}"));
        }
        if !resolved.is_dir() {
            return Ok(format!("'{path}' is a file, not a directory. Use read_file instead."));
        }

        let mut entries = Vec::new();
        if recursive {
            walk(&resolved, &ctx.workspace, &mut entries, 0)?;
        } else {
            list_one_level(&resolved, &ctx.workspace, &mut entries)?;
        }
        entries.sort();

        if entries.is_empty() {
            return Ok("(empty directory)".to_string());
        }

        // Guard against dumping an entire huge repo into the model's context.
        const MAX_ENTRIES: usize = 500;
        if entries.len() > MAX_ENTRIES {
            let shown = entries.len().min(MAX_ENTRIES);
            let mut out = entries[..shown].join("\n");
            out.push_str(&format!(
                "\n... ({} more entries truncated; narrow the path or query instead)",
                entries.len() - shown
            ));
            return Ok(out);
        }

        Ok(entries.join("\n"))
    }
}

fn list_one_level(dir: &Path, workspace: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("Failed to read directory: {}", dir.display()))? {
        let entry = entry?;
        let rel = display_relative(&entry.path(), workspace);
        let suffix = if entry.path().is_dir() { "/" } else { "" };
        out.push(format!("{rel}{suffix}"));
    }
    Ok(())
}

fn walk(dir: &Path, workspace: &Path, out: &mut Vec<String>, depth: usize) -> Result<()> {
    const MAX_DEPTH: usize = 12;
    if depth > MAX_DEPTH {
        return Ok(());
    }
    for entry in fs::read_dir(dir).with_context(|| format!("Failed to read directory: {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        if path.is_dir() {
            if IGNORED_DIR_NAMES.contains(&name) {
                continue;
            }
            out.push(format!("{}/", display_relative(&path, workspace)));
            walk(&path, workspace, out, depth + 1)?;
        } else {
            out.push(display_relative(&path, workspace));
        }
    }
    Ok(())
}

fn display_relative(path: &Path, workspace: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx(workspace: &Path) -> ToolContext {
        ToolContext {
            workspace: workspace.to_path_buf(),
        extra_read_roots: Vec::new(),
        }
    }

    #[tokio::test]
    async fn reads_existing_file_with_line_numbers() {
        let dir = std::env::temp_dir().join("rexo_test_fs_read");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("hello.txt"), "line one\nline two").unwrap();

        let tool = ReadFileTool;
        let result = tool
            .execute(&ctx(&dir), json!({"path": "hello.txt"}))
            .await
            .unwrap();
        assert!(result.contains("line one"));
        assert!(result.contains("line two"));
        assert!(result.contains('1'));
    }

    #[tokio::test]
    async fn refuses_path_outside_workspace() {
        let dir = std::env::temp_dir().join("rexo_test_fs_boundary");
        std::fs::create_dir_all(&dir).unwrap();
        let tool = ReadFileTool;
        let result = tool
            .execute(&ctx(&dir), json!({"path": "../../etc/passwd"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn lists_directory_non_recursively() {
        let dir = std::env::temp_dir().join("rexo_test_fs_list");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), "a").unwrap();

        let tool = ListFilesTool;
        let result = tool
            .execute(&ctx(&dir), json!({"path": "."}))
            .await
            .unwrap();
        assert!(result.contains("a.txt"));
        assert!(result.contains("sub/"));
    }
}
