//! Safe(r) file mutation primitives.
//!
//! Rather than letting the model overwrite whole files (easy to get wrong,
//! hard to review), [`EditFileTool`] performs an exact, unambiguous
//! find-and-replace: the model must supply the exact text it expects to
//! find, and the edit is rejected if that text doesn't appear in the file
//! exactly once. This gives us a natural conflict check for free — if the
//! file changed since the model last read it, the edit simply fails
//! instead of silently corrupting something.

use std::fs;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::security::policies::{validate_workspace_path, RiskLevel};

use super::{Tool, ToolContext, ToolPermission};

pub struct EditFileTool;

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Edit an existing text file by replacing one exact occurrence of `old_string` \
         with `new_string`. `old_string` must match the file's current content exactly \
         (including whitespace/indentation) and must be unique in the file — read the \
         file first, and include enough surrounding context to make the match unique. \
         Fails safely (no change made) if the match isn't found or isn't unique."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file, relative to the workspace root."
                },
                "old_string": {
                    "type": "string",
                    "description": "Exact text to find. Must occur exactly once in the file."
                },
                "new_string": {
                    "type": "string",
                    "description": "Text to replace it with."
                }
            },
            "required": ["path", "old_string", "new_string"]
        })
    }

    fn required_permission(&self, _ctx: &ToolContext, arguments: &Value) -> ToolPermission {
        let path = arguments.get("path").and_then(Value::as_str).unwrap_or("?");
        ToolPermission::Ask {
            summary: format!("Modify {path}"),
            risk: RiskLevel::Moderate,
        }
    }

    async fn execute(&self, ctx: &ToolContext, arguments: Value) -> Result<String> {
        let path = arguments
            .get("path")
            .and_then(Value::as_str)
            .context("Missing required argument 'path'")?;
        let old_string = arguments
            .get("old_string")
            .and_then(Value::as_str)
            .context("Missing required argument 'old_string'")?;
        let new_string = arguments
            .get("new_string")
            .and_then(Value::as_str)
            .context("Missing required argument 'new_string'")?;

        if old_string == new_string {
            return Ok("old_string and new_string are identical; nothing to do.".to_string());
        }

        let resolved = validate_workspace_path(&ctx.workspace, path)?;
        if !resolved.exists() {
            return Ok(format!(
                "File not found: {path}. Use create_file if you meant to create it."
            ));
        }

        let content = fs::read_to_string(&resolved)
            .with_context(|| format!("Failed to read file: {path}"))?;

        let occurrences = content.matches(old_string).count();
        if occurrences == 0 {
            return Ok(format!(
                "edit_file failed: old_string was not found in {path}. \
                 Re-read the file to get its exact current content before editing."
            ));
        }
        if occurrences > 1 {
            return Ok(format!(
                "edit_file failed: old_string matched {occurrences} places in {path}, \
                 but must be unique. Include more surrounding context."
            ));
        }

        let updated = content.replacen(old_string, new_string, 1);
        fs::write(&resolved, &updated).with_context(|| format!("Failed to write file: {path}"))?;

        let old_lines = old_string.lines().count();
        let new_lines = new_string.lines().count();
        Ok(format!(
            "Edited {path} (-{old_lines} / +{new_lines} lines)."
        ))
    }
}

pub struct CreateFileTool;

#[async_trait]
impl Tool for CreateFileTool {
    fn name(&self) -> &str {
        "create_file"
    }

    fn description(&self) -> &str {
        "Create a new text file with the given content. Fails if the file already exists \
         — use edit_file to modify existing files. Parent directories are created as needed."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the new file, relative to the workspace root."
                },
                "content": {
                    "type": "string",
                    "description": "Full content of the new file."
                }
            },
            "required": ["path", "content"]
        })
    }

    fn required_permission(&self, _ctx: &ToolContext, arguments: &Value) -> ToolPermission {
        let path = arguments.get("path").and_then(Value::as_str).unwrap_or("?");
        ToolPermission::Ask {
            summary: format!("Create {path}"),
            risk: RiskLevel::Moderate,
        }
    }

    async fn execute(&self, ctx: &ToolContext, arguments: Value) -> Result<String> {
        let path = arguments
            .get("path")
            .and_then(Value::as_str)
            .context("Missing required argument 'path'")?;
        let content = arguments
            .get("content")
            .and_then(Value::as_str)
            .context("Missing required argument 'content'")?;

        let resolved = validate_workspace_path(&ctx.workspace, path)?;
        if resolved.exists() {
            return Ok(format!(
                "create_file failed: {path} already exists. Use edit_file to modify it."
            ));
        }

        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create parent directories for {path}"))?;
        }
        fs::write(&resolved, content).with_context(|| format!("Failed to write file: {path}"))?;

        Ok(format!("Created {path} ({} bytes).", content.len()))
    }
}

pub struct DeleteFileTool;

#[async_trait]
impl Tool for DeleteFileTool {
    fn name(&self) -> &str {
        "delete_file"
    }

    fn description(&self) -> &str {
        "Delete a file inside the workspace. This is irreversible — always explain why \
         to the user before calling this."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to delete, relative to the workspace root."
                }
            },
            "required": ["path"]
        })
    }

    fn required_permission(&self, _ctx: &ToolContext, arguments: &Value) -> ToolPermission {
        let path = arguments.get("path").and_then(Value::as_str).unwrap_or("?");
        ToolPermission::Ask {
            summary: format!("Permanently delete {path}"),
            risk: RiskLevel::High,
        }
    }

    async fn execute(&self, ctx: &ToolContext, arguments: Value) -> Result<String> {
        let path = arguments
            .get("path")
            .and_then(Value::as_str)
            .context("Missing required argument 'path'")?;

        let resolved = validate_workspace_path(&ctx.workspace, path)?;
        if !resolved.exists() {
            return Ok(format!("File not found: {path}"));
        }
        if resolved.is_dir() {
            return Ok(format!(
                "'{path}' is a directory; delete_file only removes single files."
            ));
        }

        fs::remove_file(&resolved).with_context(|| format!("Failed to delete file: {path}"))?;
        Ok(format!("Deleted {path}."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx(workspace: &std::path::Path) -> ToolContext {
        ToolContext {
            workspace: workspace.to_path_buf(),
        extra_read_roots: Vec::new(),
        }
    }

    #[tokio::test]
    async fn edit_replaces_unique_match() {
        let dir = std::env::temp_dir().join("rexo_test_patch_edit");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f.txt"), "hello world").unwrap();

        let tool = EditFileTool;
        let result = tool
            .execute(
                &ctx(&dir),
                json!({"path": "f.txt", "old_string": "world", "new_string": "rust"}),
            )
            .await
            .unwrap();
        assert!(result.contains("Edited"));
        assert_eq!(std::fs::read_to_string(dir.join("f.txt")).unwrap(), "hello rust");
    }

    #[tokio::test]
    async fn edit_rejects_ambiguous_match() {
        let dir = std::env::temp_dir().join("rexo_test_patch_ambiguous");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f.txt"), "a a a").unwrap();

        let tool = EditFileTool;
        let result = tool
            .execute(
                &ctx(&dir),
                json!({"path": "f.txt", "old_string": "a", "new_string": "b"}),
            )
            .await
            .unwrap();
        assert!(result.contains("must be unique"));
        // File must be untouched.
        assert_eq!(std::fs::read_to_string(dir.join("f.txt")).unwrap(), "a a a");
    }

    #[tokio::test]
    async fn create_file_refuses_overwrite() {
        let dir = std::env::temp_dir().join("rexo_test_patch_create");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("exists.txt"), "old").unwrap();

        let tool = CreateFileTool;
        let result = tool
            .execute(&ctx(&dir), json!({"path": "exists.txt", "content": "new"}))
            .await
            .unwrap();
        assert!(result.contains("already exists"));
        assert_eq!(std::fs::read_to_string(dir.join("exists.txt")).unwrap(), "old");
    }

    #[tokio::test]
    async fn delete_removes_file() {
        let dir = std::env::temp_dir().join("rexo_test_patch_delete");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("gone.txt"), "bye").unwrap();

        let tool = DeleteFileTool;
        tool.execute(&ctx(&dir), json!({"path": "gone.txt"}))
            .await
            .unwrap();
        assert!(!dir.join("gone.txt").exists());
    }
}
