use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Context;
use claude_agent_sdk_rs::types::config::SettingSource;
use claude_agent_sdk_rs::types::mcp::{McpServerConfig, McpServers};
use claude_agent_sdk_rs::types::messages::ContentBlock;
use claude_agent_sdk_rs::{ClaudeAgentOptions, ClaudeClient, Message};
use futures::StreamExt;
use strum::IntoEnumIterator;

use crate::config::Config;
use crate::log::Event;
use crate::model::Model;
use crate::task::{Task, TaskId};
use crate::tool::BuiltinTool;
use crate::tools::create_tools;

/// Outcome of a single step (one execution attempt of a task).
pub enum StepOutcome {
    Success { summary: String },
    Failed { reason: String, learnings: String },
}

/// Result of a single step execution.
pub struct StepResult {
    pub task_id: TaskId,
    pub outcome: StepOutcome,
    pub model: Model,
    pub cost_usd: f64,
    pub duration: Duration,
    /// Claude Code session ID — for tracing back to CC logs or future session resume.
    pub cc_session_id: Option<String>,
}

/// Abstraction over step execution — enables mock executors in tests.
pub trait StepExecutor: Send + Sync {
    fn execute(
        &self,
        task: &Task,
        worktree_path: &Path,
        config: &Config,
        prior_learnings: &[String],
    ) -> impl Future<Output = anyhow::Result<StepResult>> + Send;
}

/// Production executor that calls the real Claude agent SDK.
pub struct RealStepExecutor;

impl StepExecutor for RealStepExecutor {
    async fn execute(
        &self,
        task: &Task,
        worktree_path: &Path,
        config: &Config,
        prior_learnings: &[String],
    ) -> anyhow::Result<StepResult> {
        execute_step(task, worktree_path, config, prior_learnings).await
    }
}

/// Build the user prompt from the task description and any prior failure learnings.
fn build_prompt(task: &Task, prior_learnings: &[String]) -> String {
    let mut prompt = task.description.clone();

    if !prior_learnings.is_empty() {
        prompt.push_str("\n\n## Previous Attempt Learnings\n");
        for learning in prior_learnings {
            use std::fmt::Write;
            write!(prompt, "\n- {learning}").expect("write to String never fails");
        }
        prompt.push('\n');
    }

    prompt
}

/// Compute the allowed and disallowed tool name lists.
///
/// Returns `(allowed, disallowed)` where both are `Vec<String>`.
/// The allowed list includes the rloop MCP tools (`complete`, `context_usage`).
fn compute_tool_lists() -> (Vec<String>, Vec<String>) {
    let allowed_builtins: HashSet<BuiltinTool> = [
        BuiltinTool::Read,
        BuiltinTool::Write,
        BuiltinTool::Edit,
        BuiltinTool::Bash,
        BuiltinTool::Glob,
        BuiltinTool::Grep,
        BuiltinTool::WebFetch,
        BuiltinTool::WebSearch,
    ]
    .into_iter()
    .collect();

    let disallowed_builtins: HashSet<BuiltinTool> = BuiltinTool::iter()
        .collect::<HashSet<_>>()
        .difference(&allowed_builtins)
        .copied()
        .collect();

    let mut allowed: Vec<String> = allowed_builtins.iter().map(ToString::to_string).collect();
    allowed.push("mcp__rloop__complete".to_string());
    allowed.push("mcp__rloop__context_usage".to_string());

    let disallowed: Vec<String> = disallowed_builtins.iter().map(ToString::to_string).collect();

    (allowed, disallowed)
}

/// Build the structured output schema used as a fallback when the agent exits
/// without calling `complete`.
fn failure_output_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "json_schema",
        "schema": {
            "type": "object",
            "properties": {
                "reason": {
                    "type": "string",
                    "description": "Why the task failed"
                },
                "learnings": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "What was learned from the failure"
                }
            },
            "required": ["reason", "learnings"]
        }
    })
}

/// Extract the outcome from the shared summary (set by the `complete` tool)
/// or from structured output on the `ResultMessage`.
fn determine_outcome(
    summary: &Mutex<Option<String>>,
    result_msg: Option<&Message>,
) -> StepOutcome {
    // Check if the `complete` tool was called and verification passed.
    if let Some(text) = summary.lock().expect("summary lock poisoned").as_ref() {
        return StepOutcome::Success {
            summary: text.clone(),
        };
    }

    // Fall back to structured output from the Result message.
    if let Some(Message::Result(res)) = result_msg
        && let Some(output) = &res.structured_output
    {
        let reason = output
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string();

        let learnings: Vec<String> = output
            .get("learnings")
            .and_then(serde_json::Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        return StepOutcome::Failed {
            reason,
            learnings: learnings.join("; "),
        };
    }

    // Neither path produced an outcome — generic failure.
    StepOutcome::Failed {
        reason: "agent exited without calling complete and produced no structured output"
            .to_string(),
        learnings: String::new(),
    }
}

/// Execute a task in a worktree: run the agent, verify, return the outcome.
///
/// This is a single attempt. Retry logic lives at the session/orchestration level.
///
/// # Errors
/// Returns an error if Claude client connection, query, or stream processing fails.
pub async fn execute_step(
    task: &Task,
    worktree_path: &Path,
    config: &Config,
    prior_learnings: &[String],
) -> anyhow::Result<StepResult> {
    let start = Instant::now();

    // 1. Build prompt
    let prompt = build_prompt(task, prior_learnings);

    // 2. Create MCP tools (complete + context_usage) and extract shared state
    let tools = create_tools(task.verification.as_ref(), worktree_path);
    let summary = tools.state.summary;
    let stop = tools.state.stop;

    let mut servers = HashMap::new();
    servers.insert("rloop".to_string(), McpServerConfig::Sdk(tools.server));

    // 3. Tool allow/disallow lists
    let (allowed_tools, disallowed_tools) = compute_tool_lists();

    // 4. Structured output schema for failure fallback
    let output_format = failure_output_schema();

    // 5. Build client options
    let model_string = task.model.to_string();
    let options = ClaudeAgentOptions::builder()
        .model(model_string)
        .cwd(worktree_path)
        .setting_sources(vec![SettingSource::Project])
        .allowed_tools(allowed_tools)
        .disallowed_tools(disallowed_tools)
        .mcp_servers(McpServers::Dict(servers))
        .max_turns(config.max_turns)
        .output_format(output_format)
        .build();

    // 6. Connect and query
    let mut client = ClaudeClient::new(options);
    client
        .connect()
        .await
        .context("failed to connect to Claude")?;
    client
        .query(&prompt)
        .await
        .context("failed to send prompt to Claude")?;

    // 7. Stream messages
    let mut cc_session_id: Option<String> = None;
    let mut result_message: Option<Message> = None;
    let mut events: Vec<Event> = Vec::new();
    let mut interrupted = false;

    {
        let mut stream = client.receive_response();
        while let Some(result) = stream.next().await {
            let msg = result.context("error receiving message from Claude stream")?;

            match &msg {
                Message::System(m) => {
                    cc_session_id = m.session_id.clone();
                }
                Message::Assistant(m) => {
                    for block in &m.message.content {
                        match block {
                            ContentBlock::Text(t) => {
                                events.push(Event::AssistantMessage {
                                    content: t.text.clone(),
                                });
                            }
                            ContentBlock::ToolUse(tu) => {
                                events.push(Event::ToolCall {
                                    name: tu.name.clone(),
                                    input: tu.input.clone(),
                                });
                            }
                            _ => {}
                        }
                    }
                }
                Message::Result(_) => {
                    result_message = Some(msg.clone());
                }
                _ => {}
            }

            // Check if the complete tool fired (stop flag set)
            if !interrupted && stop.load(Ordering::SeqCst) {
                client.interrupt().await.ok();
                interrupted = true;
                // Keep reading to receive the Result message with cost/duration.
            }
        }
    }

    client.disconnect().await.ok();

    // 8. Extract cost and session_id from Result message
    let cost_usd = if let Some(Message::Result(res)) = &result_message {
        if cc_session_id.is_none() {
            cc_session_id = Some(res.session_id.clone());
        }
        res.total_cost_usd.unwrap_or(0.0)
    } else {
        0.0
    };
    let _ = events; // Events collected for future use by SessionLogger

    // 9. Determine outcome
    let outcome = determine_outcome(&summary, result_message.as_ref());

    Ok(StepResult {
        task_id: task.id,
        outcome,
        model: task.model,
        cost_usd,
        duration: start.elapsed(),
        cc_session_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_outcome_variants() {
        let success = StepOutcome::Success {
            summary: "did the thing".into(),
        };
        assert!(matches!(success, StepOutcome::Success { summary } if summary == "did the thing"));

        let failed = StepOutcome::Failed {
            reason: "tests failed".into(),
            learnings: "need to add dep".into(),
        };
        assert!(matches!(failed, StepOutcome::Failed { reason, .. } if reason == "tests failed"));
    }

    #[test]
    fn step_result_fields() {
        let result = StepResult {
            task_id: TaskId(5),
            outcome: StepOutcome::Success { summary: "ok".into() },
            model: Model::Sonnet,
            cost_usd: 0.05,
            duration: Duration::from_secs(30),
            cc_session_id: None,
        };
        assert_eq!(result.task_id, TaskId(5));
        assert!(matches!(result.model, Model::Sonnet));
        assert!(result.cc_session_id.is_none());

        let result_with_session = StepResult {
            task_id: TaskId(3),
            outcome: StepOutcome::Failed {
                reason: "fail".into(),
                learnings: "learn".into(),
            },
            model: Model::Opus,
            cost_usd: 0.10,
            duration: Duration::from_secs(60),
            cc_session_id: Some("session-abc".into()),
        };
        assert_eq!(result_with_session.cc_session_id.as_deref(), Some("session-abc"));
    }
}
