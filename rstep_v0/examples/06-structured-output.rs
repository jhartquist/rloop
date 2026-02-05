//! Example 06: Structured Output
//!
//! Builds on ex-05 by requiring Claude to return a structured JSON result.
//!
//! What this does:
//! 1. Creates a temp directory
//! 2. Copies `templates/bug-adder/` into it
//! 3. Enables Read, Write, and Edit tools
//! 4. Asks Claude to fix the bug AND return a structured JSON result
//! 5. Parses Claude's final message to extract the JSON StepResult
//! 6. Runs `cargo test` for verification
//! 7. Displays both the parsed StepResult and verification status
//!
//! Run with:
//!   cargo run --example 06-structured-output
//!
//! Key change from ex-05:
//!   The prompt now requires Claude to output a specific JSON format.
//!   We parse and validate this structured output.

use anyhow::{anyhow, Result};
use claude_agent_sdk_rs::{ClaudeAgentOptions, ClaudeClient, Message, PermissionMode};
use colored::Colorize;
use futures::StreamExt;
use serde::Deserialize;
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;
use tracing::{debug, info};

/// Structured result from Claude
#[derive(Debug, Deserialize)]
struct StepResult {
    success: bool,
    summary: String,
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

    // Combine stdout and stderr for full output
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

        // Find all code blocks and get the last one that contains valid JSON
        let mut last_json = None;
        let mut search_text = text;

        while let Some(start_match) = re_start.find(search_text) {
            let after_start = &search_text[start_match.end()..];
            if let Some(end_match) = re_end.find(after_start) {
                let potential_json = &after_start[..end_match.start()];
                if let Ok(_) = serde_json::from_str::<serde_json::Value>(potential_json.trim()) {
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
                        if let Ok(_) = serde_json::from_str::<serde_json::Value>(potential_json) {
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
    let json_str = extract_json(text)
        .ok_or_else(|| anyhow!("No JSON found in response"))?;

    serde_json::from_str(&json_str)
        .map_err(|e| anyhow!("Failed to parse StepResult: {}", e))
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

    // Show the buggy code before Claude fixes it
    let lib_path = temp.path().join("src/lib.rs");
    println!("\n{}", "Original code (with bug):".red().bold());
    println!("{}", "-".repeat(60).dimmed());
    let original_code = fs::read_to_string(&lib_path)?;
    println!("{}", original_code.trim());
    println!("{}", "-".repeat(60).dimmed());

    // 3. Configure Claude - enable Read, Write, and Edit tools
    let options = ClaudeAgentOptions::builder()
        .model("claude-haiku-4-5-20251001")
        .allowed_tools(vec![
            "Read".to_string(),
            "Write".to_string(),
            "Edit".to_string(),
        ])
        .permission_mode(PermissionMode::BypassPermissions)
        .cwd(temp.path())
        .build();

    // 4. Create client and connect
    let mut client = ClaudeClient::new(options);
    client.connect().await?;

    // 5. Send our query with structured output requirement
    let query = r#"This is a Rust project. Read src/lib.rs, find the bug in the add function, and fix it.

After completing the task, you MUST output your result as JSON in this exact format:
{"success": true, "summary": "Brief description of what you did"}

If you couldn't complete the task:
{"success": false, "summary": "Why you couldn't complete it"}

Output the JSON as your final message."#;

    println!("\n{} {}", "Query:".yellow().bold(), "Fix bug + return JSON StepResult");
    println!(
        "{}",
        "(Claude must output structured JSON result!)"
            .green()
            .dimmed()
    );
    client.query(query).await?;

    // 6. Receive and display response
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
                Ok(message) => match &message {
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

    // 7. Disconnect Claude
    client.disconnect().await?;

    // ============================================================
    // 8. PARSE STRUCTURED OUTPUT
    // ============================================================
    println!("\n{}", "Parsing structured output...".cyan().bold());

    let step_result = match parse_step_result(&response_text) {
        Ok(result) => {
            println!("{}", "Successfully parsed StepResult!".green());
            result
        }
        Err(e) => {
            println!("{} {}", "Failed to parse:".red().bold(), e);
            println!("{}", "Raw response:".dimmed());
            println!("{}", response_text.dimmed());
            return Err(e);
        }
    };

    // Display the parsed result
    println!("\n{}", "StepResult:".blue().bold());
    println!("{}", "-".repeat(60).dimmed());
    println!(
        "  {} {}",
        "success:".dimmed(),
        if step_result.success {
            "true".green().bold()
        } else {
            "false".red().bold()
        }
    );
    println!("  {} {}", "summary:".dimmed(), step_result.summary.white());
    println!("{}", "-".repeat(60).dimmed());

    // 9. Read the file to see Claude's changes
    println!("\n{}", "Fixed code (after Claude's edit):".green().bold());
    println!("{}", "-".repeat(60).dimmed());
    let fixed_code = fs::read_to_string(&lib_path)?;
    println!("{}", fixed_code.trim());
    println!("{}", "-".repeat(60).dimmed());

    // ============================================================
    // 10. VERIFICATION - Run cargo test to verify the fix
    // ============================================================
    println!("\n{}", "Running verification...".cyan().bold());
    println!("{}", "$ cargo test".dimmed());
    println!("{}", "-".repeat(60).dimmed());

    let (passed, test_output) = run_cargo_test(temp.path())?;

    // Show test output
    println!("{}", test_output);
    println!("{}", "-".repeat(60).dimmed());

    // Display clear pass/fail status
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

    // Compare Claude's claim vs verification
    println!("\n{}", "Result comparison:".blue().bold());
    println!(
        "  {} {}",
        "Claude says:".dimmed(),
        if step_result.success { "success".green() } else { "failed".red() }
    );
    println!(
        "  {} {}",
        "Tests say:".dimmed(),
        if passed { "passed".green() } else { "failed".red() }
    );

    if step_result.success == passed {
        println!("  {} {}", "Match:".green(), "Claude's claim matches verification!");
    } else {
        println!("  {} {}", "Mismatch:".red().bold(), "Claude's claim does NOT match verification!");
    }

    // 11. Print token summary
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

    // Show which tools were used
    println!("\n{}", "Tool summary:".blue().bold());
    let read_count = tool_uses.iter().filter(|t| *t == "Read").count();
    let write_count = tool_uses.iter().filter(|t| *t == "Write").count();
    let edit_count = tool_uses.iter().filter(|t| *t == "Edit").count();
    println!("  {} {}", "Read:".dimmed(), read_count);
    println!("  {} {}", "Write:".dimmed(), write_count);
    println!("  {} {}", "Edit:".dimmed(), edit_count);

    info!("{}", "Done.".green());
    println!(
        "\n{}",
        "Next: Extend to full StepResult enum with Success/Failed/Blocked!"
            .yellow()
            .dimmed()
    );

    Ok(())
}
