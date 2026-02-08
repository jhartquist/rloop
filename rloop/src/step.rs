use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use anyhow::Context;
use claude_agent_sdk_rs::types::config::SettingSource;
use claude_agent_sdk_rs::types::mcp::{McpServerConfig, McpServers};
use claude_agent_sdk_rs::types::messages::ContentBlock;
use claude_agent_sdk_rs::{ClaudeAgentOptions, ClaudeClient, Message};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;

use crate::config::Config;
use crate::log::{Event, StreamPrinter};
use crate::model::Model;
use crate::task::{Task, TaskId};
use crate::tool::BuiltinTool;
use crate::tools::create_tools;

/// Outcome of a single step (one execution attempt of a task).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum StepOutcome {
    Success { summary: String },
    Failed { reason: String, learnings: String },
}

/// Result of a single step execution.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StepResult {
    pub task_id: TaskId,
    pub outcome: StepOutcome,
    pub model: Model,
    pub cost_usd: f64,
    #[serde(with = "duration_serde")]
    pub duration: Duration,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    /// Actual model ID string from the Claude SDK (e.g. "claude-sonnet-4-20250514").
    pub model_id: Option<String>,
    /// Claude Code session ID — for tracing back to CC logs or future session resume.
    pub cc_session_id: Option<String>,
    /// Claude stream events collected during the step.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<Event>,
}

/// Serialize `Duration` as seconds (f64) for JSON compatibility.
#[allow(clippy::missing_errors_doc)]
pub mod duration_serde {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(duration.as_secs_f64())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
        let secs = f64::deserialize(deserializer)?;
        Ok(Duration::from_secs_f64(secs))
    }
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

/// Blanket implementation for Arc<E> to enable sharing executors across async tasks.
impl<E: StepExecutor> StepExecutor for std::sync::Arc<E> {
    async fn execute(
        &self,
        task: &Task,
        worktree_path: &Path,
        config: &Config,
        prior_learnings: &[String],
    ) -> anyhow::Result<StepResult> {
        (**self).execute(task, worktree_path, config, prior_learnings).await
    }
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

/// Build the user prompt: instructions first, then task description, then learnings.
fn build_prompt(task: &Task, prior_learnings: &[String]) -> String {
    use std::fmt::Write;

    let mut prompt = String::from(
        "You are an autonomous coding agent working on a single task in an isolated git worktree.\n\
         \n\
         ## Completion\n\
         \n\
         When you have finished the task, call the `complete` tool with a brief summary.\n",
    );

    if task.verification.is_some() {
        prompt.push_str(
            "Verification commands run automatically when you call `complete`. \
             If they fail you will get the output back and can try again.\n",
        );
    }

    prompt.push_str(
        "\nIf you cannot complete the task, stop and explain the reason \
         and what you learned so future attempts can build on your work.\n",
    );

    prompt.push_str("\n## Task\n\n");
    prompt.push_str(&task.description);

    if !prior_learnings.is_empty() {
        prompt.push_str("\n\n## Previous Attempt Learnings\n");
        for learning in prior_learnings {
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

    let disallowed: Vec<String> = disallowed_builtins
        .iter()
        .map(ToString::to_string)
        .collect();

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
fn determine_outcome(summary: &Mutex<Option<String>>, result_msg: Option<&Message>) -> StepOutcome {
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

/// Extract a `TokenUsage` event from a Claude SDK usage JSON value.
fn extract_token_usage(usage: &serde_json::Value) -> Event {
    Event::TokenUsage {
        input_tokens: get_u64(usage, "input_tokens"),
        output_tokens: get_u64(usage, "output_tokens"),
        cache_read_input_tokens: get_u64(usage, "cache_read_input_tokens"),
        cache_creation_input_tokens: get_u64(usage, "cache_creation_input_tokens"),
    }
}

/// Claude models typically have a 200k token context window.
const CONTEXT_WINDOW: f64 = 200_000.0;

/// Extract a u64 from a JSON value by key, defaulting to 0.
pub fn get_u64(value: &serde_json::Value, key: &str) -> u64 {
    value
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
}

/// Tracks context window usage from the most recent assistant message.
///
/// Uses message ID deduplication to avoid counting duplicate content blocks
/// from the same API turn (the SDK sends one `Message::Assistant` per content block).
/// Context percentage is based on the input side only — per-turn output tokens
/// are unreliable placeholders in the SDK.
struct ContextTracker {
    last_message_id: Option<String>,
    last_percentage: f32,
    /// Thresholds that have already been crossed and printed.
    thresholds_crossed: [bool; 3], // 50%, 70%, 90%
}

/// Context usage thresholds for terminal alerts.
const CONTEXT_THRESHOLDS: [(usize, f32, &str); 3] = [
    (0, 50.0, "50% context used — getting full"),
    (1, 70.0, "70% context used — wrap up soon"),
    (2, 90.0, "90% context used — critical, finish immediately"),
];

impl ContextTracker {
    fn new() -> Self {
        Self {
            last_message_id: None,
            last_percentage: 0.0,
            thresholds_crossed: [false; 3],
        }
    }

    /// Update from an assistant message. Returns `true` if this was a new turn
    /// (not a duplicate content block from the same API call).
    fn update(&mut self, message_id: Option<&str>, usage: &serde_json::Value) -> bool {
        // Deduplicate by message ID — the SDK sends one Message::Assistant per
        // content block, all sharing the same message ID within a turn.
        if let Some(id) = message_id {
            if self.last_message_id.as_deref() == Some(id) {
                return false;
            }
            self.last_message_id = Some(id.to_string());
        }

        let input = get_u64(usage, "input_tokens");
        let cache_read = get_u64(usage, "cache_read_input_tokens");

        #[allow(clippy::cast_precision_loss)]
        let used = (input + cache_read) as f64;
        #[allow(clippy::cast_possible_truncation)]
        let pct = ((used / CONTEXT_WINDOW) * 100.0) as f32;
        self.last_percentage = pct;
        true
    }

    fn percentage(&self) -> f32 {
        self.last_percentage
    }

    /// Check if any new thresholds have been crossed. Returns events for
    /// newly-crossed thresholds (printed to terminal for the human watching).
    fn check_thresholds(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        for &(idx, threshold, recommendation) in &CONTEXT_THRESHOLDS {
            if !self.thresholds_crossed[idx] && self.last_percentage >= threshold {
                self.thresholds_crossed[idx] = true;
                events.push(Event::ContextUsage {
                    percentage: self.last_percentage,
                    recommendation: recommendation.to_string(),
                });
            }
        }
        events
    }
}

/// Final token counts populated from the Result message.
///
/// We do NOT accumulate per-turn values (they require deduplication and have
/// unreliable output counts). The Result message has authoritative billing totals.
#[derive(Clone, Debug, Default)]
#[allow(clippy::struct_field_names)]
struct TokenTotals {
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
}

/// Collected data from processing the Claude response stream.
struct StreamData {
    cc_session_id: Option<String>,
    model_id: Option<String>,
    result_message: Option<Message>,
    events: Vec<Event>,
    tokens: TokenTotals,
}

/// Process the Claude response stream: collect events, print to terminal, handle interrupts.
#[allow(clippy::too_many_lines)]
async fn process_stream(
    client: &mut ClaudeClient,
    stop: &std::sync::atomic::AtomicBool,
    context_pct: &AtomicU32,
    worktree_path: &Path,
    task_prefix: Option<TaskId>,
) -> anyhow::Result<StreamData> {
    let mut cc_session_id: Option<String> = None;
    let mut model_id: Option<String> = None;
    let mut result_message: Option<Message> = None;
    let mut events: Vec<Event> = Vec::new();
    let mut tokens = TokenTotals::default();
    let mut interrupted = false;
    let mut printer = StreamPrinter::new(worktree_path);
    printer.set_task_prefix(task_prefix);
    let mut context_tracker = ContextTracker::new();

    let mut stream = client.receive_response();
    while let Some(result) = stream.next().await {
        let msg = result.context("error receiving message from Claude stream")?;

        match &msg {
            Message::System(m) => {
                cc_session_id = m.session_id.clone();
                model_id = m.model.clone();
            }
            Message::Assistant(m) => {
                for block in &m.message.content {
                    match block {
                        ContentBlock::Text(t) => {
                            let event = Event::AssistantMessage {
                                content: t.text.clone(),
                            };
                            printer.print(&event);
                            events.push(event);
                        }
                        ContentBlock::ToolUse(tu) => {
                            printer.register_tool_call(&tu.id, &tu.name, &tu.input);
                            let event = Event::ToolCall {
                                name: tu.name.clone(),
                                input: tu.input.clone(),
                            };
                            printer.print(&event);
                            events.push(event);
                        }
                        _ => {}
                    }
                }

                // Update context tracker from per-message usage (deduplicated by message ID).
                if let Some(usage) = &m.message.usage {
                    let event = extract_token_usage(usage);
                    printer.print(&event);
                    events.push(event);

                    if context_tracker.update(m.message.id.as_deref(), usage) {
                        // New turn — update the shared atomic for the context_usage tool.
                        context_pct.store(context_tracker.percentage().to_bits(), Ordering::SeqCst);

                        // Check for threshold crossings and emit events for the human.
                        for threshold_event in context_tracker.check_thresholds() {
                            printer.print(&threshold_event);
                            events.push(threshold_event);
                        }
                    }
                }
            }
            Message::User(user) => {
                // Tool results arrive via the extra field (SDK quirk).
                let tool_use_id = user
                    .extra
                    .pointer("/message/content/0/tool_use_id")
                    .and_then(|v| v.as_str());

                let tool_info = tool_use_id.and_then(|id| printer.take_pending_tool(id));

                let tur = user.extra.get("tool_use_result").cloned();
                let content_text = user
                    .extra
                    .pointer("/message/content/0/content")
                    .and_then(|v| v.as_str())
                    .map(String::from);

                if tur.is_some() || content_text.is_some() {
                    let name = tool_info
                        .as_ref()
                        .map_or_else(|| "?".to_string(), |(n, _)| n.clone());
                    let output = tur
                        .or_else(|| content_text.map(serde_json::Value::String))
                        .unwrap_or(serde_json::Value::Null);
                    let event = Event::ToolResult { name, output };
                    printer.print(&event);
                    events.push(event);
                }
            }
            Message::Result(res) => {
                // Result message has authoritative billing totals — use these directly.
                if let Some(usage) = &res.usage {
                    let event = extract_token_usage(usage);
                    tokens.input_tokens = get_u64(usage, "input_tokens");
                    tokens.output_tokens = get_u64(usage, "output_tokens");
                    tokens.cache_read_tokens = get_u64(usage, "cache_read_input_tokens");
                    tokens.cache_creation_tokens = get_u64(usage, "cache_creation_input_tokens");
                    printer.print(&event);
                    events.push(event);
                }
                result_message = Some(msg.clone());
            }
            _ => {}
        }

        if !interrupted && stop.load(Ordering::SeqCst) {
            // Best-effort interrupt — stream may already be closing
            if let Err(e) = client.interrupt().await {
                eprintln!("warning: failed to interrupt Claude stream: {e}");
            }
            interrupted = true;
        }
    }

    Ok(StreamData {
        cc_session_id,
        model_id,
        result_message,
        events,
        tokens,
    })
}

/// Execute a task in a worktree: run the agent, verify, return the outcome.
///
/// This is a single attempt. Retry logic lives at the session/orchestration level.
///
/// # Errors
/// Returns an error if Claude client connection, query, or stream processing fails.
pub async fn execute_step(
    task: &Task,
    worktree_path: impl AsRef<Path>,
    config: &Config,
    prior_learnings: &[String],
) -> anyhow::Result<StepResult> {
    let worktree_path = worktree_path.as_ref();
    let start = Instant::now();

    // 1. Build prompt
    let prompt = build_prompt(task, prior_learnings);

    // 2. Create MCP tools (complete + context_usage) and extract shared state
    let tools = create_tools(
        task.verification.as_ref(),
        worktree_path,
        config.timeout_seconds,
    );
    let summary = tools.state.summary;
    let stop = tools.state.stop;
    let context_pct = tools.state.context_pct;

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

    // 7. Stream and collect events
    let task_prefix = if config.max_parallel > 1 {
        Some(task.id)
    } else {
        None
    };
    let mut data =
        process_stream(&mut client, &stop, &context_pct, worktree_path, task_prefix).await?;
    // Best-effort disconnect — session data already captured
    if let Err(e) = client.disconnect().await {
        eprintln!("warning: failed to disconnect Claude client: {e}");
    }

    // 8. Extract cost and session_id from Result message
    let cost_usd = if let Some(Message::Result(res)) = &data.result_message {
        if data.cc_session_id.is_none() {
            data.cc_session_id = Some(res.session_id.clone());
        }
        res.total_cost_usd.unwrap_or(0.0)
    } else {
        0.0
    };

    // 9. Determine outcome
    let outcome = determine_outcome(&summary, data.result_message.as_ref());

    Ok(StepResult {
        task_id: task.id,
        outcome,
        model: task.model,
        cost_usd,
        duration: start.elapsed(),
        input_tokens: data.tokens.input_tokens,
        output_tokens: data.tokens.output_tokens,
        cache_read_tokens: data.tokens.cache_read_tokens,
        cache_creation_tokens: data.tokens.cache_creation_tokens,
        model_id: data.model_id,
        cc_session_id: data.cc_session_id,
        events: data.events,
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
            outcome: StepOutcome::Success {
                summary: "ok".into(),
            },
            model: Model::Sonnet,
            cost_usd: 0.05,
            duration: Duration::from_secs(30),
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: 200,
            cache_creation_tokens: 100,
            model_id: None,
            cc_session_id: None,
            events: vec![],
        };
        assert_eq!(result.task_id, TaskId(5));
        assert_eq!(result.model, Model::Sonnet);
        assert!(result.cc_session_id.is_none());
        assert!(result.model_id.is_none());
        assert_eq!(result.input_tokens, 1000);
        assert_eq!(result.output_tokens, 500);
        assert_eq!(result.cache_read_tokens, 200);
        assert_eq!(result.cache_creation_tokens, 100);

        let result_with_session = StepResult {
            task_id: TaskId(3),
            outcome: StepOutcome::Failed {
                reason: "fail".into(),
                learnings: "learn".into(),
            },
            model: Model::Opus,
            cost_usd: 0.10,
            duration: Duration::from_secs(60),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            model_id: Some("claude-opus-4-20250514".into()),
            cc_session_id: Some("session-abc".into()),
            events: vec![],
        };
        assert_eq!(
            result_with_session.cc_session_id.as_deref(),
            Some("session-abc")
        );
        assert_eq!(
            result_with_session.model_id.as_deref(),
            Some("claude-opus-4-20250514")
        );
    }

    #[test]
    fn context_tracker_percentage_calculation() {
        let mut tracker = ContextTracker::new();

        // Build usage JSON matching the scenario from the task:
        // input=10k, cache_read=50k, cache_create=5k, output=2k
        let usage = serde_json::json!({
            "input_tokens": 10_000,
            "cache_read_input_tokens": 50_000,
            "cache_creation_input_tokens": 5_000,
            "output_tokens": 2_000,
        });

        // First update should succeed (new message)
        let updated = tracker.update(Some("msg-1"), &usage);
        assert!(updated);

        // Expected: (10k + 50k) / 200k * 100 = 30%
        // NOT: (10k + 50k + 5k + 2k) / 200k * 100 = 33.5%
        let pct = tracker.percentage();
        assert!((pct - 30.0).abs() < 0.01, "expected 30%, got {pct}%");

        // Second update with same message_id should be ignored (duplicate content block)
        let updated = tracker.update(Some("msg-1"), &usage);
        assert!(!updated);

        // Third update with new message_id should succeed
        let usage2 = serde_json::json!({
            "input_tokens": 5_000,
            "cache_read_input_tokens": 25_000,
            "cache_creation_input_tokens": 0,
            "output_tokens": 1_000,
        });
        let updated = tracker.update(Some("msg-2"), &usage2);
        assert!(updated);

        // Expected: (5k + 25k) / 200k * 100 = 15%
        let pct = tracker.percentage();
        assert!((pct - 15.0).abs() < 0.01, "expected 15%, got {pct}%");
    }
}
