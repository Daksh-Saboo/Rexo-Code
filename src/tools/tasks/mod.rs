//! A lightweight, model-driven task list — what Ctrl+T's panel shows.
//!
//! This is deliberately small: a flat list of `{id, text, done}` the
//! model can add to, check off, and clear via the `manage_tasks` tool,
//! shared with the TUI (`Agent::tasks`) so Ctrl+T can render the same
//! state the model is reading and writing. It is *not* a planner —
//! there's no automatic decomposition of a request into steps, no
//! dependency tracking between tasks, and the model isn't told to use
//! it unless it's genuinely useful for tracking a multi-step request out
//! loud. It's the same primitive Claude Code's own task tool is: a
//! visible scratchpad, not an execution engine.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

use super::{Tool, ToolContext, ToolPermission};

#[derive(Debug, Clone)]
pub struct TaskItem {
    pub id: u32,
    pub text: String,
    pub done: bool,
}

#[derive(Debug, Default)]
pub struct TaskList {
    items: Vec<TaskItem>,
    next_id: AtomicU32,
}

impl TaskList {
    fn add(&mut self, text: String) -> u32 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.items.push(TaskItem { id, text, done: false });
        id
    }

    fn set_done(&mut self, id: u32, done: bool) -> bool {
        match self.items.iter_mut().find(|t| t.id == id) {
            Some(t) => {
                t.done = done;
                true
            }
            None => false,
        }
    }

    fn remove(&mut self, id: u32) -> bool {
        let before = self.items.len();
        self.items.retain(|t| t.id != id);
        self.items.len() != before
    }

    fn clear(&mut self) {
        self.items.clear();
    }

    pub fn items(&self) -> &[TaskItem] {
        &self.items
    }

    fn render(&self) -> String {
        if self.items.is_empty() {
            return "(no tasks)".to_string();
        }
        self.items
            .iter()
            .map(|t| format!("[{}] #{} {}", if t.done { 'x' } else { ' ' }, t.id, t.text))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A `Tool` wrapping a `TaskList` shared (via `Arc<Mutex<_>>`) with
/// whoever else needs to read it — in practice, `Agent::tasks()` for the
/// TUI's Ctrl+T panel.
pub struct ManageTasksTool {
    tasks: Arc<Mutex<TaskList>>,
}

impl ManageTasksTool {
    pub fn new(tasks: Arc<Mutex<TaskList>>) -> Self {
        Self { tasks }
    }
}

#[async_trait]
impl Tool for ManageTasksTool {
    fn name(&self) -> &str {
        "manage_tasks"
    }

    fn description(&self) -> &str {
        "Track a visible to-do list for the current request — useful when a task has several \
         distinct steps and it helps the user (and you) to see what's done and what's left. \
         Not required for simple requests. Actions: 'add' (text), 'complete'/'reopen' (id), \
         'remove' (id), 'list' (no args), 'clear' (no args, removes everything)."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add", "complete", "reopen", "remove", "list", "clear"]
                },
                "text": {
                    "type": "string",
                    "description": "Task text — required for 'add'."
                },
                "id": {
                    "type": "integer",
                    "description": "Task id — required for 'complete', 'reopen', 'remove'."
                }
            },
            "required": ["action"]
        })
    }

    fn required_permission(&self, _ctx: &ToolContext, _arguments: &Value) -> ToolPermission {
        // Purely in-memory bookkeeping — never touches the filesystem or
        // runs anything, so there's nothing here for the permission
        // engine to gate.
        ToolPermission::Automatic
    }

    async fn execute(&self, _ctx: &ToolContext, arguments: Value) -> Result<String> {
        let action = arguments.get("action").and_then(Value::as_str).unwrap_or("list");
        let mut tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        match action {
            "add" => {
                let Some(text) = arguments.get("text").and_then(Value::as_str) else {
                    return Ok("manage_tasks failed: 'add' needs a 'text' argument.".to_string());
                };
                if text.trim().is_empty() {
                    return Ok("manage_tasks failed: 'text' must not be empty.".to_string());
                }
                let id = tasks.add(text.trim().to_string());
                Ok(format!("Added task #{id}.\n\n{}", tasks.render()))
            }
            "complete" | "reopen" => {
                let Some(id) = arguments.get("id").and_then(Value::as_u64) else {
                    return Ok(format!("manage_tasks failed: '{action}' needs an 'id' argument."));
                };
                let done = action == "complete";
                if tasks.set_done(id as u32, done) {
                    Ok(format!("Task #{id} marked {}.\n\n{}", if done { "done" } else { "open" }, tasks.render()))
                } else {
                    Ok(format!("manage_tasks failed: no task #{id}."))
                }
            }
            "remove" => {
                let Some(id) = arguments.get("id").and_then(Value::as_u64) else {
                    return Ok("manage_tasks failed: 'remove' needs an 'id' argument.".to_string());
                };
                if tasks.remove(id as u32) {
                    Ok(format!("Removed task #{id}.\n\n{}", tasks.render()))
                } else {
                    Ok(format!("manage_tasks failed: no task #{id}."))
                }
            }
            "clear" => {
                tasks.clear();
                Ok("Cleared all tasks.".to_string())
            }
            "list" => Ok(tasks.render()),
            other => Ok(format!("manage_tasks failed: unknown action '{other}'. Use add, complete, reopen, remove, list, or clear.")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_with_fresh_list() -> (ManageTasksTool, Arc<Mutex<TaskList>>) {
        let shared = Arc::new(Mutex::new(TaskList::default()));
        (ManageTasksTool::new(shared.clone()), shared)
    }

    fn ctx() -> ToolContext {
        ToolContext { workspace: std::env::temp_dir(), extra_read_roots: Vec::new() }
    }

    #[tokio::test]
    async fn add_then_list_shows_the_task_as_open() {
        let (tool, _) = tool_with_fresh_list();
        tool.execute(&ctx(), json!({"action": "add", "text": "write the release notes"})).await.unwrap();
        let out = tool.execute(&ctx(), json!({"action": "list"})).await.unwrap();
        assert!(out.contains("write the release notes"));
        assert!(out.contains("[ ]"));
    }

    #[tokio::test]
    async fn complete_marks_it_done() {
        let (tool, shared) = tool_with_fresh_list();
        tool.execute(&ctx(), json!({"action": "add", "text": "ship it"})).await.unwrap();
        let id = shared.lock().unwrap().items()[0].id;
        tool.execute(&ctx(), json!({"action": "complete", "id": id})).await.unwrap();
        assert!(shared.lock().unwrap().items()[0].done);
    }

    #[tokio::test]
    async fn remove_drops_the_task_entirely() {
        let (tool, shared) = tool_with_fresh_list();
        tool.execute(&ctx(), json!({"action": "add", "text": "temp"})).await.unwrap();
        let id = shared.lock().unwrap().items()[0].id;
        tool.execute(&ctx(), json!({"action": "remove", "id": id})).await.unwrap();
        assert!(shared.lock().unwrap().items().is_empty());
    }

    #[tokio::test]
    async fn complete_with_unknown_id_reports_failure_without_erroring() {
        let (tool, _) = tool_with_fresh_list();
        let out = tool.execute(&ctx(), json!({"action": "complete", "id": 999})).await.unwrap();
        assert!(out.contains("no task #999"));
    }

    #[tokio::test]
    async fn clear_empties_the_list() {
        let (tool, shared) = tool_with_fresh_list();
        tool.execute(&ctx(), json!({"action": "add", "text": "a"})).await.unwrap();
        tool.execute(&ctx(), json!({"action": "add", "text": "b"})).await.unwrap();
        tool.execute(&ctx(), json!({"action": "clear"})).await.unwrap();
        assert!(shared.lock().unwrap().items().is_empty());
    }
}
