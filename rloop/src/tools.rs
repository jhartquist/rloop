use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use claude_agent_sdk_rs::types::mcp::McpSdkServerConfig;
use claude_agent_sdk_rs::{McpToolResultContent, ToolResult, create_sdk_mcp_server, tool};
use serde_json::json;

use crate::task::VerificationCommands;

/// Create a `ToolResult` indicating an error (`is_error` = true).
fn tool_err(message: impl Into<String>) -> ToolResult {
    ToolResult {
        content: vec![McpToolResultContent::Text {
            text: message.into(),
        }],
        is_error: true,
    }
}

/// Create a `ToolResult` indicating success (`is_error` = false).
fn tool_ok(message: impl Into<String>) -> ToolResult {
    ToolResult {
        content: vec![McpToolResultContent::Text {
            text: message.into(),
        }],
        is_error: false,
    }
}

/// Shared state from the `complete` tool, readable by the caller after the session.
pub struct ToolState {
    /// Summary provided by the agent when calling `complete`.
    pub summary: Arc<Mutex<Option<String>>>,
    /// Set to `true` when `complete` succeeds -- signals step.rs to interrupt the stream.
    pub stop: Arc<AtomicBool>,
}

/// The MCP server config and its associated shared state.
pub struct Tools {
    pub server: McpSdkServerConfig,
    pub state: ToolState,
}

/// Create the `complete` and `context_usage` MCP tools, packaged as an SDK MCP server.
///
/// The `complete` tool signals task completion and triggers verification.
/// The `context_usage` tool returns the agent's current context window utilization.
#[must_use]
pub fn create_tools(
    verification: Option<&VerificationCommands>,
    worktree_path: impl AsRef<Path>,
) -> Tools {
    let worktree_path = worktree_path.as_ref();
    let summary: Arc<Mutex<Option<String>>> = Arc::default();
    let stop: Arc<AtomicBool> = Arc::default();

    let complete = build_complete_tool(
        verification.cloned(),
        worktree_path.to_path_buf(),
        Arc::clone(&summary),
        Arc::clone(&stop),
    );
    let context_usage = build_context_usage_tool();

    let server = create_sdk_mcp_server("rloop", "0.1.0", vec![complete, context_usage]);

    Tools {
        server,
        state: ToolState { summary, stop },
    }
}

/// Run each verification command in sequence. Returns `Ok(())` if all pass,
/// or `Err(message)` with the combined output of the first failure.
fn run_verification(commands: &[String], dir: &Path) -> Result<(), String> {
    for cmd in commands {
        let output = std::process::Command::new("sh")
            .args(["-c", cmd])
            .current_dir(dir)
            .output()
            .map_err(|e| format!("failed to run '{cmd}': {e}"))?;

        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "Verification failed: {cmd}\n\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
            ));
        }
    }
    Ok(())
}

fn build_complete_tool(
    verification: Option<VerificationCommands>,
    worktree_path: PathBuf,
    summary: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
) -> claude_agent_sdk_rs::SdkMcpTool {
    let schema = json!({
        "type": "object",
        "properties": {
            "summary": {
                "type": "string",
                "description": "Brief summary of what was done"
            }
        },
        "required": ["summary"]
    });

    tool!(
        "complete",
        "Call when you have finished the task.",
        schema,
        move |args: serde_json::Value| {
            let summary_ref = Arc::clone(&summary);
            let stop_ref = Arc::clone(&stop);
            let cmds = verification.clone();
            let dir = worktree_path.clone();
            async move {
                // Validate required field: summary must be present and non-empty.
                let text = match args.get("summary").and_then(|v| v.as_str()) {
                    Some(s) if !s.trim().is_empty() => s.trim().to_string(),
                    _ => {
                        return Ok(tool_err(
                            "Invalid input: 'summary' is required and must be a non-empty string.",
                        ));
                    }
                };

                // Run verification if configured
                if let Some(ref v) = cmds
                    && let Err(msg) = run_verification(&v.0, &dir)
                {
                    return Ok(tool_err(msg));
                }

                // Verification passed (or none configured) -- accept completion
                *summary_ref.lock().expect("summary lock poisoned") = Some(text);
                stop_ref.store(true, Ordering::SeqCst);

                Ok(tool_ok("Task complete."))
            }
        }
    )
}

fn build_context_usage_tool() -> claude_agent_sdk_rs::SdkMcpTool {
    let schema = json!({
        "type": "object",
        "properties": {}
    });

    tool!(
        "context_usage",
        "Check how much of your context window you have used.",
        schema,
        move |_args: serde_json::Value| {
            async move {
                let response = json!({
                    "percentage": 0.0,
                    "recommendation": "tracking not yet implemented"
                });

                Ok(tool_ok(response.to_string()))
            }
        }
    )
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
