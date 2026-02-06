use std::path::Path;

use claude_agent_sdk_rs::types::mcp::McpSdkServerConfig;

use crate::task::VerificationCommands;

/// Create the `complete` and `context_usage` MCP tools, packaged as an SDK MCP server.
///
/// The `complete` tool signals task completion and triggers verification.
/// The `context_usage` tool returns the agent's current context window utilization.
#[must_use]
pub fn create_tools(
    _verification: Option<&VerificationCommands>,
    _worktree_path: &Path,
) -> McpSdkServerConfig {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::VerificationCommands;

    #[test]
    fn create_tools_with_verification() {
        let dir = tempfile::tempdir().unwrap();
        let cmds = VerificationCommands(vec!["cargo test".into()]);
        let _server = create_tools(Some(&cmds), dir.path());
    }

    #[test]
    fn create_tools_without_verification() {
        let dir = tempfile::tempdir().unwrap();
        let _server = create_tools(None, dir.path());
    }
}
