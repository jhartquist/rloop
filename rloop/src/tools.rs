use std::path::Path;

use claude_agent_sdk_rs::types::mcp::McpSdkServerConfig;

/// Create the `complete` and `context_usage` MCP tools, packaged as an SDK MCP server.
///
/// The `complete` tool signals task completion and triggers verification.
/// The `context_usage` tool returns the agent's current context window utilization.
#[must_use]
pub fn create_tools(
    _verification_commands: Option<&[String]>,
    _worktree_path: &Path,
) -> McpSdkServerConfig {
    todo!()
}
