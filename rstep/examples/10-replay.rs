//! Example 10: Replay
//!
//! Builds on ex-09 by replaying a recorded session WITHOUT calling Claude.
//!
//! What this does:
//! 1. Loads a recording from `fixtures/recorded/`
//! 2. "Plays back" the messages as if they came from Claude
//! 3. Extracts the StepResult from the recorded response
//! 4. Applies edits from the recording to the template
//! 5. Runs verification (which should pass since the fix is baked in)
//!
//! Key benefit:
//!   - NO Claude API calls = FREE and FAST
//!   - Deterministic (same recording = same result every time)
//!   - Perfect for testing
//!
//! Run with:
//!   cargo run --example 10-replay
//!

use anyhow::{anyhow, Result};
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::BufReader;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;
use tracing::info;

/// Recording format (same as ex-09)
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
    message: serde_json::Value,
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

/// An edit operation extracted from a tool_use
#[derive(Debug)]
struct EditOperation {
    file_path: String,
    old_string: String,
    new_string: String,
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

/// Find the most recent recording file in fixtures/recorded/
fn find_latest_recording() -> Result<std::path::PathBuf> {
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/recorded");

    let mut recordings: Vec<_> = fs::read_dir(&fixtures_dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.path().extension().map_or(false, |ext| ext == "json")
                && entry.file_name().to_string_lossy().starts_with("recording-")
        })
        .collect();

    // Sort by filename (which includes timestamp, so newest is last)
    recordings.sort_by_key(|e| e.file_name());

    recordings
        .last()
        .map(|e| e.path())
        .ok_or_else(|| anyhow!("No recordings found in {:?}", fixtures_dir))
}

/// Load a recording from a JSON file
fn load_recording(path: &Path) -> Result<Recording> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let recording: Recording = serde_json::from_reader(reader)?;
    Ok(recording)
}

/// Extract Edit operations from recorded messages
fn extract_edits(messages: &[RecordedMessage]) -> Vec<EditOperation> {
    let mut edits = Vec::new();

    for msg in messages {
        if msg.message.get("type").and_then(|v| v.as_str()) == Some("assistant") {
            if let Some(content) = msg.message.get("message").and_then(|m| m.get("content")) {
                if let Some(blocks) = content.as_array() {
                    for block in blocks {
                        if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                            if block.get("name").and_then(|n| n.as_str()) == Some("Edit") {
                                if let Some(input) = block.get("input") {
                                    let file_path = input
                                        .get("file_path")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let old_string = input
                                        .get("old_string")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let new_string = input
                                        .get("new_string")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string();

                                    if !old_string.is_empty() && !new_string.is_empty() {
                                        edits.push(EditOperation {
                                            file_path,
                                            old_string,
                                            new_string,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    edits
}

/// Extract all text content from recorded assistant messages
fn extract_response_text(messages: &[RecordedMessage]) -> String {
    let mut text = String::new();

    for msg in messages {
        if msg.message.get("type").and_then(|v| v.as_str()) == Some("assistant") {
            if let Some(content) = msg.message.get("message").and_then(|m| m.get("content")) {
                if let Some(blocks) = content.as_array() {
                    for block in blocks {
                        if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                            if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                                text.push_str(t);
                                text.push('\n');
                            }
                        }
                    }
                }
            }
        }
    }

    text
}

/// Extract tool names used in the recording
fn extract_tool_uses(messages: &[RecordedMessage]) -> Vec<String> {
    let mut tools = Vec::new();

    for msg in messages {
        if msg.message.get("type").and_then(|v| v.as_str()) == Some("assistant") {
            if let Some(content) = msg.message.get("message").and_then(|m| m.get("content")) {
                if let Some(blocks) = content.as_array() {
                    for block in blocks {
                        if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                            if let Some(name) = block.get("name").and_then(|n| n.as_str()) {
                                tools.push(name.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    tools
}

/// Apply an edit to a file, converting recorded paths to local temp paths
fn apply_edit(edit: &EditOperation, temp_dir: &Path) -> Result<()> {
    // The recorded file_path is an absolute path to the original temp dir
    // We need to extract just the relative part (e.g., "src/lib.rs")
    let relative_path = if let Some(idx) = edit.file_path.rfind("/src/") {
        &edit.file_path[idx + 1..] // Get "src/..." part
    } else if let Some(idx) = edit.file_path.rfind("/Cargo.toml") {
        &edit.file_path[idx + 1..]
    } else {
        // Fall back to just the filename
        Path::new(&edit.file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&edit.file_path)
    };

    let local_path = temp_dir.join(relative_path);

    let content = fs::read_to_string(&local_path)?;
    let new_content = content.replace(&edit.old_string, &edit.new_string);

    if content == new_content {
        return Err(anyhow!(
            "Edit had no effect - old_string not found in {}",
            local_path.display()
        ));
    }

    fs::write(&local_path, new_content)?;
    Ok(())
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

    // ============================================================
    // 1. LOAD RECORDING
    // ============================================================
    println!(
        "\n{}",
        "+----------------------------------------------------------+"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|                    REPLAY MODE                           |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|              No Claude API calls - FREE!                 |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "+----------------------------------------------------------+"
            .cyan()
            .bold()
    );

    let recording_path = find_latest_recording()?;
    println!(
        "\n{} {}",
        "Loading recording:".yellow().bold(),
        recording_path.display().to_string().white()
    );

    let recording = load_recording(&recording_path)?;

    println!("\n{}", "Recording Metadata:".blue().bold());
    println!("{}", "-".repeat(60).dimmed());
    println!(
        "  {} {}",
        "Recorded at:".dimmed(),
        recording.metadata.recorded_at.white()
    );
    println!(
        "  {} {}",
        "Model:".dimmed(),
        recording.metadata.model.cyan()
    );
    println!(
        "  {} {}",
        "Task:".dimmed(),
        recording.metadata.task.white()
    );
    println!(
        "  {} {}",
        "Total messages:".dimmed(),
        recording.messages.len().to_string().white().bold()
    );
    println!("{}", "-".repeat(60).dimmed());

    // ============================================================
    // 2. SET UP WORKING DIRECTORY
    // ============================================================
    let temp = tempdir()?;
    info!("{}", "Setting up working directory...".cyan().bold());

    let template_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates/bug-adder");
    copy_dir_all(&template_dir, temp.path())?;
    info!(
        "{} {}",
        "Copied template:".green(),
        "bug-adder".white().bold()
    );

    // Show the buggy code before replay
    let lib_path = temp.path().join("src/lib.rs");
    println!("\n{}", "Original code (with bug):".red().bold());
    println!("{}", "-".repeat(60).dimmed());
    let original_code = fs::read_to_string(&lib_path)?;
    println!("{}", original_code.trim());
    println!("{}", "-".repeat(60).dimmed());

    // ============================================================
    // 3. REPLAY MESSAGES
    // ============================================================
    println!("\n{}", "Replaying messages...".yellow().bold());
    println!("{}", "-".repeat(60).dimmed());

    let mut assistant_count = 0;
    let mut result_count = 0;
    let mut system_count = 0;

    for (i, msg) in recording.messages.iter().enumerate() {
        let msg_type = msg.message.get("type").and_then(|v| v.as_str()).unwrap_or("unknown");

        match msg_type {
            "assistant" => {
                assistant_count += 1;
                print!("  {} ", format!("[{}]", i).dimmed());
                print!("{} ", "ASSISTANT".green());

                // Show text content preview
                if let Some(content) = msg.message.get("message").and_then(|m| m.get("content")) {
                    if let Some(blocks) = content.as_array() {
                        for block in blocks {
                            match block.get("type").and_then(|t| t.as_str()) {
                                Some("text") => {
                                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                        let preview: String = text.chars().take(50).collect();
                                        let preview = preview.replace('\n', " ");
                                        print!("{}", preview.dimmed());
                                        if text.len() > 50 {
                                            print!("{}", "...".dimmed());
                                        }
                                    }
                                }
                                Some("tool_use") => {
                                    if let Some(name) = block.get("name").and_then(|n| n.as_str()) {
                                        print!("{} {}", "->".magenta(), name.magenta());
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                println!();
            }
            "result" => {
                result_count += 1;
                print!("  {} ", format!("[{}]", i).dimmed());
                println!("{}", "RESULT".blue().bold());
            }
            "system" => {
                system_count += 1;
                // Don't print system messages (too noisy)
            }
            _ => {}
        }
    }

    println!("{}", "-".repeat(60).dimmed());
    println!(
        "\n{} {} assistant, {} result, {} system",
        "Replayed:".green().bold(),
        assistant_count.to_string().white(),
        result_count.to_string().white(),
        system_count.to_string().white()
    );

    // ============================================================
    // 4. EXTRACT AND APPLY EDITS
    // ============================================================
    let edits = extract_edits(&recording.messages);
    let tool_uses = extract_tool_uses(&recording.messages);

    if !tool_uses.is_empty() {
        println!("\n{}", "Tools used in recording:".magenta().bold());
        for tool in &tool_uses {
            println!("  {} {}", "->".magenta(), tool);
        }
    }

    if !edits.is_empty() {
        println!(
            "\n{} {}",
            "Applying edits from recording:".cyan().bold(),
            edits.len().to_string().white()
        );

        for (i, edit) in edits.iter().enumerate() {
            print!("  {} ", format!("[{}]", i + 1).dimmed());
            match apply_edit(edit, temp.path()) {
                Ok(()) => {
                    println!(
                        "{} {}",
                        "Applied edit to".green(),
                        edit.file_path.split('/').last().unwrap_or(&edit.file_path)
                    );
                }
                Err(e) => {
                    println!("{} {}", "Failed to apply edit:".red(), e);
                }
            }
        }
    }

    // Show the fixed code after applying edits
    println!("\n{}", "Fixed code (after replay edits):".green().bold());
    println!("{}", "-".repeat(60).dimmed());
    let fixed_code = fs::read_to_string(&lib_path)?;
    println!("{}", fixed_code.trim());
    println!("{}", "-".repeat(60).dimmed());

    // ============================================================
    // 5. EXTRACT STEP RESULT
    // ============================================================
    let response_text = extract_response_text(&recording.messages);

    println!("\n{}", "Extracting StepResult from recording...".cyan().bold());

    let step_result = match parse_step_result(&response_text) {
        Ok(result) => {
            println!("{}", "Successfully extracted StepResult!".green());
            result
        }
        Err(e) => {
            println!("{} {}", "Failed to extract:".red().bold(), e);
            println!("{}", "Response text from recording:".dimmed());
            println!("{}", response_text.dimmed());
            return Err(e);
        }
    };

    display_step_result(&step_result);

    // ============================================================
    // 6. VERIFICATION
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
    // 7. SUMMARY
    // ============================================================
    println!("\n{}", "Replay Summary:".blue().bold());
    println!("{}", "-".repeat(60).dimmed());
    println!(
        "  {} {}",
        "Recording:".dimmed(),
        recording_path.file_name().unwrap().to_string_lossy().cyan()
    );
    println!(
        "  {} {}",
        "Messages replayed:".dimmed(),
        recording.messages.len().to_string().white().bold()
    );
    println!(
        "  {} {} assistant, {} result, {} system",
        "Breakdown:".dimmed(),
        assistant_count.to_string().white(),
        result_count.to_string().white(),
        system_count.to_string().white()
    );
    println!(
        "  {} {}",
        "Edits applied:".dimmed(),
        edits.len().to_string().white()
    );
    println!(
        "  {} {}",
        "Result:".dimmed(),
        step_result.as_str().green().bold()
    );
    println!(
        "  {} {}",
        "Verification:".dimmed(),
        if passed {
            "PASSED".green().bold()
        } else {
            "FAILED".red().bold()
        }
    );
    println!(
        "  {} {}",
        "API calls:".dimmed(),
        "0 (FREE!)".cyan().bold()
    );
    println!("{}", "-".repeat(60).dimmed());

    info!("{}", "Replay complete.".green());

    Ok(())
}
