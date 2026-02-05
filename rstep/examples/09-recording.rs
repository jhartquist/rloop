//! Example 09: Recording
//!
//! Builds on ex-08 by recording ALL Claude messages to a fixture file for replay.
//!
//! What this does:
//! 1. Creates a temp directory
//! 2. Copies `templates/bug-adder/` into it
//! 3. Enables Read, Write, and Edit tools
//! 4. Asks Claude to fix the bug
//! 5. Captures EVERY message from the stream as raw JSON
//! 6. Saves the recording to fixtures/recorded/recording-{timestamp}.json
//! 7. Runs verification
//! 8. Displays recording summary
//!
//! Key difference from session logging:
//!   - Session logging captures events for analysis (processed/structured)
//!   - Recording captures EXACT messages for replay (raw JSON, deterministic tests)
//!
//! Run with:
//!   cargo run --example 09-recording
//!
//! The recording can be replayed later without calling Claude (ex-10).

use anyhow::{anyhow, Result};
use chrono::Utc;
use claude_agent_sdk_rs::{ClaudeAgentOptions, ClaudeClient, Message, PermissionMode};
use colored::Colorize;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;
use tracing::{debug, info};

/// Recording format for replay
#[derive(Debug, Serialize, Deserialize)]
struct Recording {
    metadata: RecordingMetadata,
    messages: Vec<RecordedMessage>,
}

/// Metadata about the recording session
#[derive(Debug, Serialize, Deserialize)]
struct RecordingMetadata {
    recorded_at: String,
    model: String,
    task: String,
}

/// A single recorded message with timestamp
#[derive(Debug, Serialize, Deserialize)]
struct RecordedMessage {
    timestamp: String,
    message: serde_json::Value, // Store the raw message JSON
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

/// Convert a Message to serde_json::Value for recording
fn message_to_value(message: &Message) -> serde_json::Value {
    match message {
        Message::Assistant(msg) => {
            serde_json::json!({
                "type": "assistant",
                "message": msg.message,
            })
        }
        Message::Result(result) => {
            serde_json::json!({
                "type": "result",
                "duration_ms": result.duration_ms,
                "duration_api_ms": result.duration_api_ms,
                "is_error": result.is_error,
                "num_turns": result.num_turns,
                "usage": result.usage,
            })
        }
        Message::System(sys) => {
            serde_json::json!({
                "type": "system",
                "subtype": sys.subtype,
                "cwd": sys.cwd,
                "session_id": sys.session_id,
            })
        }
        // Other message types we don't need for replay
        _ => {
            serde_json::json!({
                "type": "other",
            })
        }
    }
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
    // 3. SET UP RECORDING
    // ============================================================
    let task = "Fix the bug in the add function";
    let model = "claude-haiku-4-5-20251001";

    let mut recording = Recording {
        metadata: RecordingMetadata {
            recorded_at: Utc::now().to_rfc3339(),
            model: model.to_string(),
            task: task.to_string(),
        },
        messages: Vec::new(),
    };

    println!(
        "\n{} {}",
        "Recording:".cyan().bold(),
        "Capturing all messages for replay".white()
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

    // 7. Receive and display response, recording ALL messages
    println!("\n{}", "Response:".green().bold());
    println!("{}", "-".repeat(60).dimmed());

    let mut total_input_tokens: u64 = 0;
    let mut total_output_tokens: u64 = 0;
    let mut response_text = String::new();
    let mut tool_uses: Vec<String> = Vec::new();

    {
        let mut stream = client.receive_response();
        while let Some(result) = stream.next().await {
            match result {
                Ok(message) => {
                    // ============================================================
                    // RECORD THE RAW MESSAGE
                    // ============================================================
                    let recorded = RecordedMessage {
                        timestamp: Utc::now().to_rfc3339(),
                        message: message_to_value(&message),
                    };
                    recording.messages.push(recorded);

                    // Process the message for display (same as before)
                    match &message {
                        Message::Assistant(msg) => {
                            for block in &msg.message.content {
                                match block {
                                    claude_agent_sdk_rs::ContentBlock::Text(text_block) => {
                                        response_text.push_str(&text_block.text);
                                    }
                                    claude_agent_sdk_rs::ContentBlock::ToolUse(tool_use) => {
                                        tool_uses.push(tool_use.name.clone());
                                    }
                                    _ => {}
                                }
                            }

                            if let Some(usage) = &msg.message.usage {
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
                    }
                }
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

    // ============================================================
    // 12. SAVE RECORDING TO FIXTURES
    // ============================================================
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/recorded");
    fs::create_dir_all(&fixtures_dir)?;

    let timestamp = Utc::now().format("%Y%m%d-%H%M%S");
    let recording_filename = format!("recording-{}.json", timestamp);
    let recording_path = fixtures_dir.join(&recording_filename);

    let file = File::create(&recording_path)?;
    let writer = BufWriter::new(file);
    serde_json::to_writer_pretty(writer, &recording)?;

    println!("\n{}", "Recording Saved:".blue().bold());
    println!("{}", "-".repeat(60).dimmed());
    println!(
        "  {} {}",
        "File:".dimmed(),
        recording_path.display().to_string().cyan()
    );
    println!(
        "  {} {}",
        "Messages:".dimmed(),
        recording.messages.len().to_string().white().bold()
    );
    println!(
        "  {} {}",
        "Model:".dimmed(),
        recording.metadata.model.white()
    );
    println!(
        "  {} {}",
        "Task:".dimmed(),
        recording.metadata.task.white()
    );
    println!(
        "  {} {}",
        "Result:".dimmed(),
        step_result.as_str().green()
    );
    println!("{}", "-".repeat(60).dimmed());

    // Show message types recorded
    println!("\n{}", "Messages recorded:".yellow().bold());
    let mut assistant_count = 0;
    let mut result_count = 0;
    let mut system_count = 0;

    for msg in &recording.messages {
        if let Some(msg_type) = msg.message.get("type").and_then(|v| v.as_str()) {
            match msg_type {
                "assistant" => assistant_count += 1,
                "result" => result_count += 1,
                "system" => system_count += 1,
                _ => {}
            }
        }
    }

    println!("  {} assistant messages", assistant_count.to_string().white());
    println!("  {} result messages", result_count.to_string().white());
    if system_count > 0 {
        println!("  {} system messages", system_count.to_string().white());
    }

    // 13. Print token summary
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
        "Next: Replay from recorded fixtures!".yellow().dimmed()
    );

    Ok(())
}
