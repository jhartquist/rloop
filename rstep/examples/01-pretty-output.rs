//! Example 01: Pretty Formatted Output
//!
//! Builds on ex-00 but with human-readable, colored output.
//!
//! What this does:
//! 1. Creates a temp directory (clean slate, no CLAUDE.md)
//! 2. Calls Claude with "What is 2+2?"
//! 3. Prints formatted, colored output with clear sections
//! 4. Shows token counts in a readable summary
//! 5. Cleans up temp directory on exit
//!
//! Run with:
//!   cargo run --example 01-pretty-output
//!
//! Control verbosity with RUST_LOG:
//!   RUST_LOG=debug cargo run --example 01-pretty-output

use anyhow::Result;
use claude_agent_sdk_rs::{ClaudeAgentOptions, ClaudeClient, Message, PermissionMode};
use colored::Colorize;
use futures::StreamExt;
use tempfile::tempdir;
use tracing::{debug, info};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing with env filter (RUST_LOG controls verbosity)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_target(false)
        .init();

    // 1. Create temp directory
    let temp = tempdir()?;
    info!("{}", "Starting Claude query...".cyan().bold());
    debug!("Working directory: {:?}", temp.path());

    // 2. Configure with maximum control
    let options = ClaudeAgentOptions::builder()
        .model("claude-haiku-4-5-20251001")
        .allowed_tools(Vec::<String>::new()) // No tools
        .permission_mode(PermissionMode::BypassPermissions)
        .cwd(temp.path())
        .build();

    // 3. Create client and connect
    let mut client = ClaudeClient::new(options);
    client.connect().await?;

    // 4. Send our query
    let query = "What is 2+2?";
    println!("\n{} {}", "Query:".yellow().bold(), query);
    client.query(query).await?;

    // 5. Receive and display response
    println!("\n{}", "Response:".green().bold());
    println!("{}", "─".repeat(40).dimmed());

    let mut total_input_tokens: u64 = 0;
    let mut total_output_tokens: u64 = 0;
    let mut response_text = String::new();

    {
        let mut stream = client.receive_response();
        while let Some(result) = stream.next().await {
            match result {
                Ok(message) => {
                    match &message {
                        Message::Assistant(msg) => {
                            // Extract text content from the message
                            for block in &msg.message.content {
                                if let claude_agent_sdk_rs::ContentBlock::Text(text_block) = block {
                                    response_text.push_str(&text_block.text);
                                }
                            }

                            // Track tokens
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
                            debug!("Assistant message received");
                        }
                        Message::Result(result) => {
                            // Final usage from result
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
                            debug!("Result received");
                        }
                        _ => {
                            debug!("Other message type received");
                        }
                    }
                }
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
    println!("{}", "─".repeat(40).dimmed());

    // 6. Disconnect
    client.disconnect().await?;

    // 7. Print token summary
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

    Ok(())
}
