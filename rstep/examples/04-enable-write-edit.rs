//! Example 04: Enable Write and Edit Tools
//!
//! Builds on ex-03 by enabling Write and Edit tools so Claude can modify files.
//!
//! What this does:
//! 1. Creates a temp directory
//! 2. Copies `templates/bug-adder/` into it
//! 3. Enables Read, Write, and Edit tools
//! 4. Asks Claude "Fix the bug in the add function"
//! 5. Claude reads the file, finds the bug (a - b instead of a + b), and fixes it!
//! 6. After Claude finishes, we read the file to verify the fix
//!
//! Run with:
//!   cargo run --example 04-enable-write-edit
//!
//! Key change from ex-03:
//!   .allowed_tools(vec!["Read"])  ->  .allowed_tools(vec!["Read", "Write", "Edit"])

use anyhow::Result;
use claude_agent_sdk_rs::{ClaudeAgentOptions, ClaudeClient, Message, PermissionMode};
use colored::Colorize;
use futures::StreamExt;
use std::fs;
use std::path::Path;
use tempfile::tempdir;
use tracing::{debug, info};

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
    println!("{}", "─".repeat(60).dimmed());
    let original_code = fs::read_to_string(&lib_path)?;
    println!("{}", original_code.trim());
    println!("{}", "─".repeat(60).dimmed());

    // 3. Configure Claude - enable Read, Write, and Edit tools!
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

    // 5. Send our query - ask Claude to fix the bug
    let query =
        "This is a Rust project. Read src/lib.rs, find the bug in the add function, and fix it.";
    println!("\n{} {}", "Query:".yellow().bold(), query);
    println!(
        "{}",
        "(Claude now has Read, Write, and Edit tools!)"
            .green()
            .dimmed()
    );
    client.query(query).await?;

    // 6. Receive and display response
    println!("\n{}", "Response:".green().bold());
    println!("{}", "─".repeat(60).dimmed());

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
            println!("  {} {}", "→".magenta(), tool);
        }
        println!();
    }

    // Print the response text
    if !response_text.is_empty() {
        println!("{}", response_text);
    }
    println!("{}", "─".repeat(60).dimmed());

    // 7. Disconnect
    client.disconnect().await?;

    // 8. Read the file again to verify the fix
    println!("\n{}", "Fixed code (after Claude's edit):".green().bold());
    println!("{}", "─".repeat(60).dimmed());
    let fixed_code = fs::read_to_string(&lib_path)?;
    println!("{}", fixed_code.trim());
    println!("{}", "─".repeat(60).dimmed());

    // Check if the bug was fixed
    let bug_fixed = fixed_code.contains("a + b") && !fixed_code.contains("a - b");
    if bug_fixed {
        println!(
            "\n{} {}",
            "SUCCESS:".green().bold(),
            "Claude fixed the bug! (a - b → a + b)".white()
        );
    } else {
        println!(
            "\n{} {}",
            "INCOMPLETE:".yellow().bold(),
            "Bug may not be fully fixed. Check the code above.".white()
        );
    }

    // 9. Print token summary
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
        "Next: Add verification - run cargo test to confirm the fix!"
            .yellow()
            .dimmed()
    );

    Ok(())
}
