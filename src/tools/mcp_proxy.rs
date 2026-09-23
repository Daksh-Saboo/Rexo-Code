//! Wraps one MCP-server-discovered tool as a real REXO [`Tool`] — the
//! bridge between [`crate::mcp::McpClient`] (the protocol) and the
//! agent's tool-use loop (which only knows how to call `Tool`s).

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use crate::mcp::McpClient;
use crate::security::policies::RiskLevel;
use crate::tools::{Tool, ToolContext, ToolPermission};

pub struct McpProxyTool {
    client: McpClient,
    /// The name as the MCP server itself knows it (what goes in the
    /// `tools/call` request) — distinct from `rexo_name`, which is what
    /// the model sees and calls REXO-side.
    mcp_tool_name: String,
    rexo_name: String,
    description: String,
    schema: Value,
}

impl McpProxyTool {
    pub fn new(client: McpClient, server_name: &str, info: crate::mcp::McpToolInfo) -> Self {
        // "mcp__<server>__<tool>" — same double-underscore namespacing
        // this very kind of proxy tool uses elsewhere, and it keeps two
        // servers that happen to both expose a tool called e.g. "search"
        // from colliding in REXO's single flat tool namespace.
        let rexo_name = format!("mcp__{server_name}__{}", info.name);
        Self { client, mcp_tool_name: info.name, rexo_name, description: info.description, schema: info.input_schema }
    }
}

#[async_trait]
impl Tool for McpProxyTool {
    fn name(&self) -> &str {
        &self.rexo_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn schema(&self) -> Value {
        self.schema.clone()
    }

    fn required_permission(&self, _ctx: &ToolContext, _arguments: &Value) -> ToolPermission {
        // An MCP tool runs arbitrary, server-defined code with unknown
        // side effects REXO has no way to classify further — the same
        // "ask by default" posture as a shell command, not the
        // automatic pass-through read-only tools get.
        ToolPermission::Ask { summary: format!("Call MCP tool `{}` on `{}`", self.mcp_tool_name, self.client.server_name), risk: RiskLevel::Moderate }
    }

    async fn execute(&self, _ctx: &ToolContext, arguments: Value) -> Result<String> {
        match self.client.call_tool(&self.mcp_tool_name, arguments).await {
            Ok(text) => Ok(text),
            Err(e) => Ok(format!("MCP tool call failed: {e}")),
        }
    }
}
