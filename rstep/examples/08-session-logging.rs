//! Example 08: Session Logging
//!
//! Builds on ex-07 by logging all session events to a JSONL file.
//!
//! What this does:
//! 1. Creates a temp directory
//! 2. Copies `templates/bug-adder/` into it
//! 3. Creates a session.jsonl file for logging events
//! 4. Logs session_start event
//! 5. Enables Read, Write, and Edit tools
//! 6. Asks Claude to fix the bug
//! 7. Logs assistant_message, tool_use, and tool_result events as they occur
//! 8. Parses Claude's final message for StepResult
//! 9. Runs verification
//! 10. Logs session_end event with totals
//! 11. Displays sample log content
//!
//! Run with:
//!   cargo run --example 08-session-logging
//!
//! Key change from ex-07:
//!   All events are logged to session.jsonl in JSONL format (one JSON per line)
//!   for later analysis and replay.

use anyhow::{anyhow, Result};
use chrono::Utc;
use claude_agent_sdk_rs::{ClaudeAgentOptions, ClaudeClient, Message, PermissionMode};
use colored::Colorize;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;
use tracing::{debug, info};

/// Session events logged to JSONL
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum SessionEvent {
    #[serde(rename = "session_start")]
    SessionStart {
        timestamp: String,
        task: String,
        model: String,
    },
    #[serde(rename = "assistant_message")]
    AssistantMessage {
        timestamp: String,
        content: String,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        timestamp: String,
        tool_name: String,
        tool_id: String,
    },
    #[serde(rename = "session_end")]
    SessionEnd {
        timestamp: String,
        total_input_tokens: u64,
        total_output_tokens: u64,
        result: String,
    },
}

/// Structured result from Claude - enum with tagged variants
#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "result")]
enum StepResult {
    #[serde(rename = "success")]
    Success { summary: String },
    #[serde(rename = "failed")]
    Failed {
        reason: String,
        next_steps: Vec<String>,
    },
    #[serde(rename = "blocked")]
    Blocked { blocker: String },
}

impl StepResult {
    fn as_str(&self) -> &'static str {
        match self {
            StepResult::Success { .. } => "success",
            StepResult::Failed { .. } => "failed",
            StepResult::Blocked { .. } => "blocked",
        }
    }
}

/// Session logger that writes events to JSONL
struct SessionLogger {
    writer: BufWriter<File>,
}

impl SessionLogger {
    fn new(path: &Path) -> Result<Self> {
        let file = File::create(path)?;
        Ok(Self {
            writer: BufWriter::new(file),
        })
    }

    fn log(&mut self, event: &SessionEvent) -> Result<()> {
        let json = serde_json::to_string(event)?;
        writeln!(self.writer, "{}", json)?;
        self.writer.flush()?;
        Ok(())
    }
}

/// Recursively copy a directory and all its contents
fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if ty.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Run cargo test in the given directory and return (success, output)
fn run_cargo_test(dir: &Path) -> Result<(bool, String)> {
    let output = Command::new("cargo")
        .arg("test")
        .current_dir(dir)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let full_output = format!("{}{}", stdout, stderr);

    Ok((output.status.success(), full_output))
}

/// Extract JSON from text, handling code blocks and finding the last occurrence
fn extract_json(text: &str) -> Option<String> {
    // Try to find JSON in code blocks first (```json ... ``` or ``` ... ```)
    let code_block_patterns = [
        (r"```json\s*", r"\s*```"),
        (r"```\s*", r"\s*```"),
    ];

    for (start_pattern, end_pattern) in code_block_patterns {
        let re_start = regex::Regex::new(start_pattern).ok()?;
        let re_end = regex::Regex::new(end_pattern).ok()?;

        let mut last_json = None;
        let mut search_text = text;

        while let Some(start_match) = re_start.find(search_text) {
            let after_start = &search_text[start_match.end()..];
            if let Some(end_match) = re_end.find(after_start) {
                let potential_json = &after_start[..end_match.start()];
                if serde_json::from_str::<serde_json::Value>(potential_json.trim()).is_ok() {
                    last_json = Some(potential_json.trim().to_string());
                }
                search_text = &after_start[end_match.end()..];
            } else {
                break;
            }
        }

        if last_json.is_some() {
            return last_json;
        }
    }

    // Fall back to finding raw JSON objects (last occurrence)
    let mut last_json = None;
    let mut depth = 0;
    let mut start_idx = None;

    for (i, c) in text.char_indices() {
        match c {
            '{' => {
                if depth == 0 {
                    start_idx = Some(i);
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(start) = start_idx {
                        let potential_json = &text[start..=i];
                        if serde_json::from_str::<serde_json::Value>(potential_json).is_ok() {
                            last_json = Some(potential_json.to_string());
                        }
                    }
                    start_idx = None;
                }
            }
            _ => {}
        }
    }

    last_json
}

/// Parse StepResult from Claude's response text
fn parse_step_result(text: &str) -> Result<StepResult> {
    let json_str =
        extract_json(text).ok_or_else(|| anyhow!("No JSON found in response"))?;

    serde_json::from_str(&json_str).map_err(|e| anyhow!("Failed to parse StepResult: {}", e))
}

/// Display the StepResult with appropriate coloring
fn display_step_result(result: &StepResult) {
    println!("\n{}", "StepResult:".blue().bold());
    println!("{}", "-".repeat(60).dimmed());

    match result {
        StepResult::Success { summary } => {
            println!(
                "  {} {}",
                "result:".dimmed(),
                "SUCCESS".green().bold()
            );
            println!("  {} {}", "summary:".dimmed(), summary.white());
        }
        StepResult::Failed { reason, next_steps } => {
            println!(
                "  {} {}",
                "result:".dimmed(),
                "FAILED".red().bold()
            );
            println!("  {} {}", "reason:".dimmed(), reason.white());
            if !next_steps.is_empty() {
                println!("  {}", "next_steps:".dimmed());
                for step in next_steps {
                    println!("    {} {}", "-".dimmed(), step);
                }
            }
        }
        StepResult::Blocked { blocker } => {
            println!(
                "  {} {}",
                "result:".dimmed(),
                "BLOCKED".yellow().bold()
            );
            println!("  {} {}", "blocker:".dimmed(), blocker.white());
        }
    }

    println!("{}", "-".repeat(60).dimmed());
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_target(false)
        .init();

    // 1. Create temp directory
    let temp = tempdir()?;
    info!("{}", "Setting up working directory...".cyan().bold());

    // 2. Copy template into temp directory
    let template_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates/bug-adder");
    debug!("Template source: {:?}", template_dir);
    debug!("Working directory: {:?}", temp.path());

    copy_dir_all(&template_dir, temp.path())?;
    info!(
        "{} {}",
        "Copied template:".green(),
        "bug-adder".white().bold()
    );

    // ============================================================
    // 3. SET UP SESSION LOGGING
    // ============================================================
    let session_log_path = temp.path().join("session.jsonl");
    let mut logger = SessionLogger::new(&session_log_path)?;
    info!(
        "{} {}",
        "Session log:".cyan(),
        session_log_path.display().to_string().white()
    );

    let task = "Fix the bug in the add function";
    let model = "claude-haiku-4-5-20251001";

    // Log session start
    logger.log(&SessionEvent::SessionStart {
        timestamp: Utc::now().to_rfc3339(),
        task: task.to_string(),
        model: model.to_string(),
    })?;
    println!(
        "  {} {}",
        "Logged:".dimmed(),
        "session_start".green()
    );

    // Show the buggy code before Claude fixes it
    let lib_path = temp.path().join("src/lib.rs");
    println!("\n{}", "Original code (with bug):".red().bold());
    println!("{}", "-".repeat(60).dimmed());
    let original_code = fs::read_to_string(&lib_path)?;
    println!("{}", original_code.trim());
    println!("{}", "-".repeat(60).dimmed());

    // 4. Configure Claude - enable Read, Write, and Edit tools
    let options = ClaudeAgentOptions::builder()
        .model(model)
        .allowed_tools(vec![
            "Read".to_string(),
            "Write".to_string(),
            "Edit".to_string(),
        ])
        .permission_mode(PermissionMode::BypassPermissions)
        .cwd(temp.path())
        .build();

    // 5. Create client and connect
    let mut client = ClaudeClient::new(options);
    client.connect().await?;

    // 6. Send our query with structured output requirement
    let query = r#"This is a Rust project. Read src/lib.rs, find the bug in the add function, and fix it.

After completing the task, you MUST output your result as JSON in one of these exact formats:

If successful:
{"result": "success", "summary": "What you did"}

If failed:
{"result": "failed", "reason": "Why it failed", "next_steps": ["suggestion 1", "suggestion 2"]}

If blocked (need human input):
{"result": "blocked", "blocker": "What you need from the human"}

Output the JSON as your final message."#;

    println!(
        "\n{} {}",
        "Query:".yellow().bold(),
        "Fix bug + return JSON StepResult enum"
    );
    client.query(query).await?;

    // 7. Receive and display response, logging events
    println!("\n{}", "Response:".green().bold());
    println!("{}", "-".repeat(60).dimmed());

    let mut total_input_tokens: u64 = 0;
    let mut total_output_tokens: u64 = 0;
    let mut response_text = String::new();
    let mut tool_uses: Vec<String> = Vec::new();
    let mut event_counts = EventCounts::default();

    {
        let mut stream = client.receive_response();
        while let Some(result) = stream.next().await {
            match result {
                Ok(message) => match &message {
                    Message::Assistant(msg) => {
                        let mut message_content = String::new();
                        let mut msg_input_tokens: Option<u64> = None;
                        let mut msg_output_tokens: Option<u64> = None;

                        for block in &msg.message.content {
                            match block {
                                claude_agent_sdk_rs::ContentBlock::Text(text_block) => {
                                    message_content.push_str(&text_block.text);
                                    response_text.push_str(&text_block.text);
                                }
                                claude_agent_sdk_rs::ContentBlock::ToolUse(tool_use) => {
                                    tool_uses.push(tool_use.name.clone());
                                    // Log tool use event
                                    logger.log(&SessionEvent::ToolUse {
                                        timestamp: Utc::now().to_rfc3339(),
                                        tool_name: tool_use.name.clone(),
                                        tool_id: tool_use.id.clone(),
                                    })?;
                                    event_counts.tool_use += 1;
                                }
                                _ => {}
                            }
                        }

                        if let Some(usage) = &msg.message.usage {
                            if let Some(input) =
                                usage.get("input_tokens").and_then(|v| v.as_u64())
                            {
                                total_input_tokens += input;
                                msg_input_tokens = Some(input);
                            }
                            if let Some(output) =
                                usage.get("output_tokens").and_then(|v| v.as_u64())
                            {
                                total_output_tokens += output;
                                msg_output_tokens = Some(output);
                            }
                        }

                        // Log assistant message event (if there's text content)
                        if !message_content.is_empty() {
                            logger.log(&SessionEvent::AssistantMessage {
                                timestamp: Utc::now().to_rfc3339(),
                                content: message_content,
                                input_tokens: msg_input_tokens,
                                output_tokens: msg_output_tokens,
                            })?;
                            event_counts.assistant_message += 1;
                        }
                    }
                    Message::Result(result) => {
                        if let Some(usage) = &result.usage {
                            if let Some(input) =
                                usage.get("input_tokens").and_then(|v| v.as_u64())
                            {
                                total_input_tokens += input;
                            }
                            if let Some(output) =
                                usage.get("output_tokens").and_then(|v| v.as_u64())
                            {
                                total_output_tokens += output;
                            }
                        }
                    }
                    _ => {}
                },
                Err(e) => {
                    eprintln!("{} {}", "Error:".red().bold(), e);
                }
            }
        }
    }

    // Show tool uses
    if !tool_uses.is_empty() {
        println!("{}", "Tools used:".magenta().bold());
        for tool in &tool_uses {
            println!("  {} {}", "->".magenta(), tool);
        }
        println!();
    }

    // Print the response text
    if !response_text.is_empty() {
        println!("{}", response_text);
    }
    println!("{}", "-".repeat(60).dimmed());

    // 8. Disconnect Claude
    client.disconnect().await?;

    // ============================================================
    // 9. PARSE STRUCTURED OUTPUT
    // ============================================================
    println!("\n{}", "Parsing structured output...".cyan().bold());

    let step_result = match parse_step_result(&response_text) {
        Ok(result) => {
            println!("{}", "Successfully parsed StepResult enum!".green());
            result
        }
        Err(e) => {
            println!("{} {}", "Failed to parse:".red().bold(), e);
            println!("{}", "Raw response:".dimmed());
            println!("{}", response_text.dimmed());
            return Err(e);
        }
    };

    display_step_result(&step_result);

    // 10. Read the file to see Claude's changes
    println!("\n{}", "Fixed code (after Claude's edit):".green().bold());
    println!("{}", "-".repeat(60).dimmed());
    let fixed_code = fs::read_to_string(&lib_path)?;
    println!("{}", fixed_code.trim());
    println!("{}", "-".repeat(60).dimmed());

    // ============================================================
    // 11. VERIFICATION
    // ============================================================
    println!("\n{}", "Running verification...".cyan().bold());
    println!("{}", "$ cargo test".dimmed());
    println!("{}", "-".repeat(60).dimmed());

    let (passed, test_output) = run_cargo_test(temp.path())?;

    println!("{}", test_output);
    println!("{}", "-".repeat(60).dimmed());

    if passed {
        println!(
            "\n{}",
            "+----------------------------------------------------------+"
                .green()
                .bold()
        );
        println!(
            "{}",
            "|                  VERIFICATION PASSED                     |"
                .green()
                .bold()
        );
        println!(
            "{}",
            "+----------------------------------------------------------+"
                .green()
                .bold()
        );
    } else {
        println!(
            "\n{}",
            "+----------------------------------------------------------+"
                .red()
                .bold()
        );
        println!(
            "{}",
            "|                  VERIFICATION FAILED                     |"
                .red()
                .bold()
        );
        println!(
            "{}",
            "+----------------------------------------------------------+"
                .red()
                .bold()
        );
    }

    // Determine final result string
    let result_str = if passed {
        step_result.as_str()
    } else {
        "verification_failed"
    };

    // ============================================================
    // 12. LOG SESSION END
    // ============================================================
    logger.log(&SessionEvent::SessionEnd {
        timestamp: Utc::now().to_rfc3339(),
        total_input_tokens,
        total_output_tokens,
        result: result_str.to_string(),
    })?;
    println!(
        "\n  {} {}",
        "Logged:".dimmed(),
        "session_end".green()
    );

    // ============================================================
    // 13. DISPLAY SESSION LOG SUMMARY
    // ============================================================
    println!("\n{}", "Session Log Summary:".blue().bold());
    println!("{}", "-".repeat(60).dimmed());
    println!(
        "  {} {}",
        "Log file:".dimmed(),
        session_log_path.display().to_string().cyan()
    );
    println!("  {}", "Events logged:".dimmed());
    println!("    {} session_start", "1".white());
    println!(
        "    {} assistant_message",
        event_counts.assistant_message.to_string().white()
    );
    println!(
        "    {} tool_use",
        event_counts.tool_use.to_string().white()
    );
    println!("    {} session_end", "1".white());
    let total_events = 1 + event_counts.assistant_message + event_counts.tool_use + 1;
    println!(
        "  {} {}",
        "Total:".dimmed(),
        total_events.to_string().cyan().bold()
    );
    println!("{}", "-".repeat(60).dimmed());

    // ============================================================
    // 14. SHOW SAMPLE LOG CONTENT
    // ============================================================
    println!("\n{}", "Sample log content (first 5 lines):".yellow().bold());
    println!("{}", "-".repeat(60).dimmed());
    let log_content = fs::read_to_string(&session_log_path)?;
    for (i, line) in log_content.lines().take(5).enumerate() {
        // Pretty print with jq-like formatting for display
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
            let event_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("unknown");
            println!(
                "  {} {} {}",
                format!("{}.", i + 1).dimmed(),
                event_type.cyan(),
                format!("({})", line.len()).dimmed()
            );
        }
    }
    println!("{}", "-".repeat(60).dimmed());

    // Tip for querying
    println!("\n{}", "Query with jq:".yellow().dimmed());
    println!(
        "  {}",
        format!("cat {} | jq .", session_log_path.display()).white()
    );
    println!(
        "  {}",
        format!(
            "cat {} | jq 'select(.type == \"tool_use\")'",
            session_log_path.display()
        )
        .white()
    );

    // 15. Print token summary
    println!("\n{}", "Tokens:".blue().bold());
    println!(
        "  {} {}",
        "Input:".dimmed(),
        total_input_tokens.to_string().white()
    );
    println!(
        "  {} {}",
        "Output:".dimmed(),
        total_output_tokens.to_string().white()
    );
    println!(
        "  {} {}",
        "Total:".dimmed(),
        (total_input_tokens + total_output_tokens)
            .to_string()
            .cyan()
            .bold()
    );

    info!("{}", "Done.".green());
    println!(
        "\n{}",
        "Next: Recording sessions to fixtures for replay!".yellow().dimmed()
    );

    // Note: temp directory (including session.jsonl) is cleaned up on drop
    // In a real implementation, you'd copy the log somewhere persistent first

    Ok(())
}

/// Track event counts for summary
#[derive(Default)]
struct EventCounts {
    assistant_message: usize,
    tool_use: usize,
}
