//! Example 03: Enable Read Tool
//!
//! Builds on ex-02 by enabling the Read tool so Claude can actually see files.
//!
//! What this does:
//! 1. Creates a temp directory
//! 2. Copies `templates/bug-adder/` into it
//! 3. Enables the Read tool (but no other tools)
//! 4. Asks Claude "What does the add function do?"
//! 5. Claude can now read src/lib.rs and describe what it sees!
//!
//! Run with:
//!   cargo run --example 03-enable-read-tool
//!
//! Key change from ex-02:
//!   .allowed_tools(Vec::<String>::new())  ->  .allowed_tools(vec!["Read".to_string()])

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

    // List what we copied
    println!("\n{}", "Project files:".yellow().bold());
    for entry in fs::read_dir(temp.path())? {
        let entry = entry?;
        let name = entry.file_name();
        if entry.file_type()?.is_dir() {
            println!("  {}/", name.to_string_lossy().cyan());
        } else {
            println!("  {}", name.to_string_lossy().dimmed());
        }
    }

    // 3. Configure Claude - enable Read tool!
    let options = ClaudeAgentOptions::builder()
        .model("claude-haiku-4-5-20251001")
        .allowed_tools(vec!["Read".to_string()]) // Enable Read tool
        .permission_mode(PermissionMode::BypassPermissions)
        .cwd(temp.path())
        .build();

    // 4. Create client and connect
    let mut client = ClaudeClient::new(options);
    client.connect().await?;

    // 5. Send our query
    // Be specific about where to look - Claude needs guidance on the directory structure
    let query = "This is a Rust project. Read src/lib.rs and tell me what the add function does.";
    println!("\n{} {}", "Query:".yellow().bold(), query);
    println!(
        "{}",
        "(Claude now has Read tool - it can read files!)".green().dimmed()
    );
    client.query(query).await?;

    // 6. Receive and display response
    println!("\n{}", "Response:".green().bold());
    println!("{}", "─".repeat(60).dimmed());

    let mut total_input_tokens: u64 = 0;
    let mut total_output_tokens: u64 = 0;
    let mut response_text = String::new();
    let mut tool_uses = Vec::new();

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
                                    tool_uses.push(format!(
                                        "{} (id: {})",
                                        tool_use.name, tool_use.id
                                    ));
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

    // 8. Print token summary
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
        "Next: Enable Write and Edit tools so Claude can fix the bug!"
            .yellow()
            .dimmed()
    );

    Ok(())
}
