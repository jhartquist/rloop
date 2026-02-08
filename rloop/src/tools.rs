use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use claude_agent_sdk_rs::types::mcp::McpSdkServerConfig;
use claude_agent_sdk_rs::{McpToolResultContent, ToolResult, create_sdk_mcp_server, tool};
use serde_json::json;
use wait_timeout::ChildExt;

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

/// Shared state from the MCP tools, readable by the caller after the session.
pub struct ToolState {
    /// Summary provided by the agent when calling `complete`.
    pub summary: Arc<Mutex<Option<String>>>,
    /// Set to `true` when `complete` succeeds -- signals step.rs to interrupt the stream.
    pub stop: Arc<AtomicBool>,
    /// Current context usage percentage, stored as f32 bits in a u32 for atomic access.
    pub context_pct: Arc<AtomicU32>,
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
/// `timeout_seconds` controls how long each verification command may run (0 = no timeout).
#[must_use]
pub fn create_tools(
    verification: Option<&VerificationCommands>,
    worktree_path: impl AsRef<Path>,
    timeout_seconds: u64,
) -> Tools {
    let worktree_path = worktree_path.as_ref();
    let summary: Arc<Mutex<Option<String>>> = Arc::default();
    let stop: Arc<AtomicBool> = Arc::default();
    let context_pct: Arc<AtomicU32> = Arc::default();

    let timeout = if timeout_seconds == 0 {
        None
    } else {
        Some(Duration::from_secs(timeout_seconds))
    };

    let complete = build_complete_tool(
        verification.cloned(),
        worktree_path.to_path_buf(),
        Arc::clone(&summary),
        Arc::clone(&stop),
        timeout,
    );
    let context_usage = build_context_usage_tool(Arc::clone(&context_pct));

    let server = create_sdk_mcp_server("rloop", "0.1.0", vec![complete, context_usage]);

    Tools {
        server,
        state: ToolState {
            summary,
            stop,
            context_pct,
        },
    }
}

/// Run each verification command in sequence. Returns `Ok(())` if all pass,
/// or `Err(message)` with the combined output of the first failure or timeout.
fn run_verification(
    commands: &[String],
    dir: &Path,
    timeout: Option<Duration>,
) -> Result<(), String> {
    for cmd in commands {
        let mut child = std::process::Command::new("sh")
            .args(["-c", cmd])
            .current_dir(dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to run '{cmd}': {e}"))?;

        let status = if let Some(dur) = timeout {
            match child.wait_timeout(dur) {
                Ok(Some(s)) => s,
                Ok(None) => {
                    // Timed out — kill and reap. Errors here are non-fatal since
                    // the process may have already exited between timeout and kill.
                    if let Err(e) = child.kill() {
                        eprintln!("warning: failed to kill timed-out verification process: {e}");
                    }
                    if let Err(e) = child.wait() {
                        eprintln!("warning: failed to reap timed-out verification process: {e}");
                    }
                    return Err(format!(
                        "Verification timed out after {}s: {cmd}",
                        dur.as_secs()
                    ));
                }
                Err(e) => return Err(format!("failed to wait on '{cmd}': {e}")),
            }
        } else {
            child
                .wait()
                .map_err(|e| format!("failed to wait on '{cmd}': {e}"))?
        };

        if !status.success() {
            // Best-effort pipe reads — partial output is still useful for diagnostics
            let stdout = child
                .stdout
                .map(|mut r| {
                    let mut s = String::new();
                    if let Err(e) = std::io::Read::read_to_string(&mut r, &mut s) {
                        eprintln!("warning: failed to read verification stdout: {e}");
                    }
                    s
                })
                .unwrap_or_default();
            let stderr = child
                .stderr
                .map(|mut r| {
                    let mut s = String::new();
                    if let Err(e) = std::io::Read::read_to_string(&mut r, &mut s) {
                        eprintln!("warning: failed to read verification stderr: {e}");
                    }
                    s
                })
                .unwrap_or_default();
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
    timeout: Option<Duration>,
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
            let cmd_timeout = timeout;
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
                    && let Err(msg) = run_verification(v, &dir, cmd_timeout)
                {
                    return Ok(tool_err(msg));
                }

                // Verification passed (or none configured) -- accept completion
                match summary_ref.lock() {
                    Ok(mut guard) => {
                        *guard = Some(text);
                        stop_ref.store(true, Ordering::SeqCst);
                        Ok(tool_ok("Task complete."))
                    }
                    Err(_) => Ok(tool_err("Internal error: summary lock poisoned")),
                }
            }
        }
    )
}

fn build_context_usage_tool(context_pct: Arc<AtomicU32>) -> claude_agent_sdk_rs::SdkMcpTool {
    let schema = json!({
        "type": "object",
        "properties": {}
    });

    tool!(
        "context_usage",
        "Check how much of your context window you have used.",
        schema,
        move |_args: serde_json::Value| {
            let pct_ref = Arc::clone(&context_pct);
            async move {
                let percentage = f32::from_bits(pct_ref.load(Ordering::SeqCst));

                let recommendation = if percentage < 50.0 {
                    "plenty of room"
                } else if percentage < 70.0 {
                    "getting full, consider wrapping up"
                } else if percentage < 90.0 {
                    "running low, wrap up soon"
                } else {
                    "critical, finish immediately"
                };

                let response = json!({
                    "percentage": percentage,
                    "recommendation": recommendation,
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
        let cmds = VerificationCommands::new(vec!["cargo test".into()]);
        let _server = create_tools(Some(&cmds), dir.path(), 300);
    }

    #[test]
    fn create_tools_without_verification() {
        let dir = tempfile::tempdir().unwrap();
        let _server = create_tools(None, dir.path(), 300);
    }

    #[test]
    fn verification_passes_without_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let cmds = vec!["true".to_string()];
        assert!(run_verification(&cmds, dir.path(), None).is_ok());
    }

    #[test]
    fn verification_fails_without_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let cmds = vec!["false".to_string()];
        let err = run_verification(&cmds, dir.path(), None).unwrap_err();
        assert!(err.contains("Verification failed"));
    }

    #[test]
    fn verification_passes_within_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let cmds = vec!["true".to_string()];
        let timeout = Some(Duration::from_secs(5));
        assert!(run_verification(&cmds, dir.path(), timeout).is_ok());
    }

    #[test]
    fn verification_times_out() {
        let dir = tempfile::tempdir().unwrap();
        // sleep for 10 seconds, but timeout after 1
        let cmds = vec!["sleep 10".to_string()];
        let timeout = Some(Duration::from_secs(1));
        let start = std::time::Instant::now();
        let err = run_verification(&cmds, dir.path(), timeout).unwrap_err();
        let elapsed = start.elapsed();
        assert!(err.contains("timed out"));
        assert!(err.contains("1s"));
        // Should finish close to 1 second, not 10
        assert!(elapsed < Duration::from_secs(5));
    }

    #[test]
    fn verification_timeout_zero_means_no_timeout() {
        let dir = tempfile::tempdir().unwrap();
        // timeout_seconds=0 is converted to None by create_tools,
        // but test run_verification directly with None
        let cmds = vec!["true".to_string()];
        assert!(run_verification(&cmds, dir.path(), None).is_ok());
    }

    #[test]
    fn verification_stops_at_first_failure() {
        let dir = tempfile::tempdir().unwrap();
        // Second command should never run
        let cmds = vec!["false".to_string(), "true".to_string()];
        let err = run_verification(&cmds, dir.path(), Some(Duration::from_secs(5))).unwrap_err();
        assert!(err.contains("Verification failed: false"));
    }

    #[test]
    fn verification_stops_at_first_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let cmds = vec!["sleep 10".to_string(), "true".to_string()];
        let timeout = Some(Duration::from_secs(1));
        let err = run_verification(&cmds, dir.path(), timeout).unwrap_err();
        assert!(err.contains("timed out"));
    }

    #[test]
    fn verification_multiple_commands_all_pass() {
        let dir = tempfile::tempdir().unwrap();
        let cmds = vec!["true".to_string(), "true".to_string(), "true".to_string()];
        let timeout = Some(Duration::from_secs(5));
        assert!(run_verification(&cmds, dir.path(), timeout).is_ok());
    }

    /// Verify that `ToolState` shared summary is safe under concurrent access.
    #[test]
    fn tool_state_concurrent_summary_access() {
        let summary: Arc<Mutex<Option<String>>> = Arc::default();
        let stop: Arc<AtomicBool> = Arc::default();

        let mut handles = Vec::new();

        // Spawn 10 writer threads
        for i in 0..10 {
            let s = Arc::clone(&summary);
            let st = Arc::clone(&stop);
            handles.push(std::thread::spawn(move || {
                *s.lock().unwrap() = Some(format!("summary-{i}"));
                st.store(true, Ordering::SeqCst);
            }));
        }

        // Spawn 10 reader threads
        for _ in 0..10 {
            let s = Arc::clone(&summary);
            let st = Arc::clone(&stop);
            handles.push(std::thread::spawn(move || {
                let _ = s.lock().unwrap().clone();
                let _ = st.load(Ordering::SeqCst);
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // After all threads complete, summary should be set to one of the values
        let final_val = summary.lock().unwrap();
        assert!(final_val.is_some());
        assert!(final_val.as_ref().unwrap().starts_with("summary-"));
    }

    /// Verify empty verification command list passes.
    #[test]
    fn verification_empty_commands_passes() {
        let dir = tempfile::tempdir().unwrap();
        let cmds: Vec<String> = vec![];
        assert!(run_verification(&cmds, dir.path(), None).is_ok());
    }
}
