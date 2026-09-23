//! Tool abstraction layer.
//!
//! Every capability the agent can invoke (reading a file, running a shell
//! command, editing code, ...) is implemented as a [`Tool`]. Tools are
//! registered in a [`ToolRegistry`], which is also responsible for turning
//! them into OpenAI-compatible JSON-schema function definitions that get
//! sent to the model.
//!
//! Tools do **not** decide on their own whether they are allowed to run.
//! They only report what kind of permission a given invocation would need
//! via [`Tool::required_permission`]. The actual approval flow lives in
//! [`crate::security::permissions`], and is orchestrated by the agent loop
//! in [`crate::agent`]. This separation keeps tools simple/testable and
//! keeps all "is this allowed?" policy in one place.

pub mod filesystem;
pub mod git;
pub mod mcp_proxy;
pub mod patch;
pub mod search;
pub mod tasks;
pub mod terminal;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;

use crate::security::policies::RiskLevel;

/// Shared, read-only context handed to every tool invocation.
#[derive(Clone, Debug)]
pub struct ToolContext {
    /// Absolute, canonicalized path to the project root. Tools that touch
    /// the filesystem must keep every access inside this boundary.
    pub workspace: PathBuf,
    /// Additional directories added via `/add-dir` that read-only tools
    /// (`read_file`, `list_files`) may also resolve an *absolute* path
    /// into — deliberately not extended to `edit_file`/`create_file`/
    /// `delete_file`/`run_command`/git-write, which stay scoped to
    /// `workspace` alone. See `security::policies::validate_read_path`.
    pub extra_read_roots: Vec<PathBuf>,
}

/// What has to happen before a specific tool call is allowed to execute.
#[derive(Debug, Clone)]
pub enum ToolPermission {
    /// Safe to run without asking (read_file, list_files, search_files, ...).
    Automatic,
    /// Needs the user's (or config's) sign-off before running.
    Ask { summary: String, risk: RiskLevel },
    /// Never allowed, regardless of config or user input (e.g. a path
    /// escaping the workspace, or a command on the hard-coded deny list).
    Denied { reason: String },
}

#[async_trait]
pub trait Tool: Send + Sync {
    /// Stable machine name, e.g. "read_file". Must match `^[a-zA-Z0-9_]+$`
    /// since it is used verbatim as an OpenAI-style function name.
    fn name(&self) -> &str;

    /// Human/model-facing description of what the tool does and when to
    /// use it. Shown to the model as part of the tool schema.
    fn description(&self) -> &str;

    /// JSON schema (the `parameters` object of an OpenAI function
    /// definition) describing the arguments this tool accepts.
    fn schema(&self) -> Value;

    /// Determine what permission the given arguments would require. Called
    /// *before* `execute`, so it must not perform side effects.
    fn required_permission(&self, ctx: &ToolContext, arguments: &Value) -> ToolPermission {
        let _ = (ctx, arguments);
        ToolPermission::Automatic
    }

    /// Run the tool and return a plain-text result that will be sent back
    /// to the model as a `tool` message. Errors are turned into a textual
    /// explanation rather than aborting the whole agent loop, so the model
    /// gets a chance to recover (e.g. try a different path).
    async fn execute(&self, ctx: &ToolContext, arguments: Value) -> Result<String>;
}

/// OpenAI-compatible `{"type": "function", "function": {...}}` definition,
/// serialized and sent to the model as part of the `tools` array.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: FunctionDefinition,
}

#[derive(Debug, Clone, Serialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// Central registry: register tools, look them up by name, list them, and
/// generate the schema array sent to the provider.
#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
        }
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn list(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.values().cloned().collect()
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|t| ToolDefinition {
                kind: "function",
                function: FunctionDefinition {
                    name: t.name().to_string(),
                    description: t.description().to_string(),
                    parameters: t.schema(),
                },
            })
            .collect()
    }

    /// Convenience used by the agent loop: look the tool up and surface a
    /// friendly error (turned into a tool-result message, not a crash) if
    /// the model hallucinated a tool name that doesn't exist.
    pub fn require(&self, name: &str) -> Result<Arc<dyn Tool>> {
        self.get(name)
            .ok_or_else(|| anyhow!("Unknown tool '{name}'. Available tools: {}", self.names_joined()))
    }

    fn names_joined(&self) -> String {
        self.tools.keys().cloned().collect::<Vec<_>>().join(", ")
    }
}

/// Builds the default registry containing every tool REXO ships with.
pub fn default_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(filesystem::ReadFileTool));
    registry.register(Arc::new(filesystem::ListFilesTool));
    registry.register(Arc::new(search::SearchFilesTool));
    registry.register(Arc::new(patch::EditFileTool));
    registry.register(Arc::new(patch::CreateFileTool));
    registry.register(Arc::new(patch::DeleteFileTool));
    registry.register(Arc::new(terminal::RunCommandTool));
    registry.register(Arc::new(git::GitTool));
    registry
}
