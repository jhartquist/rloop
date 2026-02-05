//! Example 02: Working Directory Setup
//!
//! Builds on ex-01 by copying a template project into the temp directory.
//!
//! What this does:
//! 1. Creates a temp directory
//! 2. Copies `templates/bug-adder/` into it
//! 3. Calls Claude asking about the project (but Claude can't read files yet!)
//! 4. Shows that we need tools for Claude to actually see the code
//!
//! Run with:
//!   cargo run --example 02-working-dir-setup
//!
//! Note: Claude can't actually see the files - it can only respond based on
//! the question. This example demonstrates setting up the working directory;
//! enabling tools comes in later examples.

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

    // 3. Configure Claude - still no tools!
    let options = ClaudeAgentOptions::builder()
        .model("claude-haiku-4-5-20251001")
        .allowed_tools(Vec::<String>::new()) // No tools - Claude can't read files
        .permission_mode(PermissionMode::BypassPermissions)
        .cwd(temp.path())
        .build();

    // 4. Create client and connect
    let mut client = ClaudeClient::new(options);
    client.connect().await?;

    // 5. Send our query
    // Note: Claude can't actually see the files, it can only respond based on the question
    let query = "Look at this Rust project. What does the add function do?";
    println!("\n{} {}", "Query:".yellow().bold(), query);
    println!(
        "{}",
        "(Note: Claude has no tools - it cannot read files yet!)".dimmed()
    );
    client.query(query).await?;

    // 6. Receive and display response
    println!("\n{}", "Response:".green().bold());
    println!("{}", "─".repeat(60).dimmed());

    let mut total_input_tokens: u64 = 0;
    let mut total_output_tokens: u64 = 0;
    let mut response_text = String::new();

    {
        let mut stream = client.receive_response();
        while let Some(result) = stream.next().await {
            match result {
                Ok(message) => match &message {
                    Message::Assistant(msg) => {
                        for block in &msg.message.content {
                            if let claude_agent_sdk_rs::ContentBlock::Text(text_block) = block {
                                response_text.push_str(&text_block.text);
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
        "Next: Enable the Read tool so Claude can actually see the files!"
            .yellow()
            .dimmed()
    );

    Ok(())
}
