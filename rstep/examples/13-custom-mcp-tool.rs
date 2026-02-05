//! Example 13: Custom MCP Tool
//!
//! Demonstrates adding custom MCP tools that Claude can invoke.
//!
//! What this does:
//! 1. Creates an in-process MCP server with a custom `step_complete` tool
//! 2. Sets up Claude with both standard tools and our custom MCP tool
//! 3. Asks Claude to fix the bug and call `step_complete` when done
//! 4. Claude can now call our custom tool which we handle in-process!
//!
//! The SDK supports custom MCP tools via:
//! - `create_sdk_mcp_server()` - Creates an in-process MCP server
//! - `tool!` macro - Ergonomic way to define tools
//! - `McpServers::Dict` - Passes MCP servers to the client
//!
//! Run with:
//!   cargo run --example 13-custom-mcp-tool
//!
//! Key additions from previous examples:
//!   - Define custom tools with `tool!` macro
//!   - Create an MCP server with `create_sdk_mcp_server()`
//!   - Pass to client via `.mcp_servers(McpServers::Dict(...))`

use anyhow::Result;
use claude_agent_sdk_rs::{
    create_sdk_mcp_server, tool, ClaudeAgentOptions, ClaudeClient, McpServerConfig, McpServers,
    Message, PermissionMode, ToolResult,
};
use claude_agent_sdk_rs::types::mcp::ToolResultContent as McpToolResultContent;
use colored::Colorize;
use futures::StreamExt;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tempfile::tempdir;
use tracing::{debug, info};

// ============================================================
// TYPES
// ============================================================

/// Arguments for the step_complete tool
#[derive(Debug, Deserialize)]
struct StepCompleteArgs {
    /// Whether the task was successful
    success: bool,
    /// Summary of what was done
    summary: String,
    /// Optional reason for failure (if not successful)
    #[serde(default)]
    reason: Option<String>,
}

// ============================================================
// HELPERS
// ============================================================

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

// ============================================================
// MAIN
// ============================================================

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

    println!(
        "\n{}",
        "+----------------------------------------------------------+"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|                CUSTOM MCP TOOL EXAMPLE                   |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|                                                          |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|  Claude can now call our custom `step_complete` tool!    |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "+----------------------------------------------------------+"
            .cyan()
            .bold()
    );

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

    // ============================================================
    // 3. CREATE CUSTOM MCP SERVER WITH TOOLS
    // ============================================================

    println!("\n{}", "Creating custom MCP server...".cyan().bold());

    // Track whether step_complete was called
    let step_completed = Arc::new(AtomicBool::new(false));
    let step_completed_clone = step_completed.clone();

    // Captured result from the tool call
    let captured_result: Arc<std::sync::Mutex<Option<StepCompleteArgs>>> =
        Arc::new(std::sync::Mutex::new(None));
    let captured_result_clone = captured_result.clone();

    // Define our custom tool using the `tool!` macro
    let step_complete_tool = tool!(
        "step_complete",
        "Signal that the task step is complete. Call this when you have finished the task, \
         whether successful or failed. This is the preferred way to indicate completion.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "success": {
                    "type": "boolean",
                    "description": "Whether the task was completed successfully"
                },
                "summary": {
                    "type": "string",
                    "description": "Brief summary of what was accomplished or attempted"
                },
                "reason": {
                    "type": "string",
                    "description": "If success is false, the reason for failure"
                }
            },
            "required": ["success", "summary"]
        }),
        move |args: serde_json::Value| {
            let completed = step_completed_clone.clone();
            let result_storage = captured_result_clone.clone();
            async move {
                // Parse the arguments
                let parsed: StepCompleteArgs = serde_json::from_value(args)?;

                println!("\n{}", "=".repeat(60).magenta());
                println!(
                    "{}",
                    "  CUSTOM TOOL CALLED: step_complete".magenta().bold()
                );
                println!("{}", "=".repeat(60).magenta());
                println!("  {} {}", "success:".dimmed(), parsed.success);
                println!("  {} {}", "summary:".dimmed(), parsed.summary);
                if let Some(ref reason) = parsed.reason {
                    println!("  {} {}", "reason:".dimmed(), reason);
                }
                println!("{}", "=".repeat(60).magenta());

                // Mark as completed
                completed.store(true, Ordering::SeqCst);

                // Store the result for later inspection
                *result_storage.lock().unwrap() = Some(StepCompleteArgs {
                    success: parsed.success,
                    summary: parsed.summary.clone(),
                    reason: parsed.reason.clone(),
                });

                // Return success response
                Ok(ToolResult {
                    content: vec![McpToolResultContent::Text {
                        text: format!(
                            "Step completion recorded. Success: {}, Summary: {}",
                            parsed.success, parsed.summary
                        ),
                    }],
                    is_error: false,
                })
            }
        }
    );

    // Create the MCP server with our tools
    let mcp_server = create_sdk_mcp_server("rstep-tools", "0.1.0", vec![step_complete_tool]);

    println!(
        "  {} {}",
        "Server:".dimmed(),
        "rstep-tools v0.1.0".white()
    );
    println!("  {} {}", "Tools:".dimmed(), "step_complete".cyan());

    // Build the MCP servers map
    let mut mcp_servers_map: HashMap<String, McpServerConfig> = HashMap::new();
    mcp_servers_map.insert("rstep-tools".to_string(), McpServerConfig::Sdk(mcp_server));

    // ============================================================
    // 4. CONFIGURE CLAUDE WITH STANDARD + CUSTOM TOOLS
    // ============================================================

    let options = ClaudeAgentOptions::builder()
        .model("claude-haiku-4-5-20251001")
        .allowed_tools(vec![
            "Read".to_string(),
            "Write".to_string(),
            "Edit".to_string(),
            // MCP tools are automatically available when server is connected
            // They appear as "mcp__rstep-tools__step_complete"
        ])
        .mcp_servers(McpServers::Dict(mcp_servers_map))
        .permission_mode(PermissionMode::BypassPermissions)
        .cwd(temp.path())
        .build();

    // 5. Create client and connect
    let mut client = ClaudeClient::new(options);
    client.connect().await?;

    // 6. Send our query - ask Claude to use our custom tool
    let query = r#"This is a Rust project. Read src/lib.rs, find the bug in the add function, and fix it.

After fixing the bug, you MUST call the `mcp__rstep-tools__step_complete` tool to signal completion:
- If you successfully fixed the bug, call it with success=true and a summary of what you fixed
- If you couldn't fix it, call it with success=false, a summary, and a reason

Do NOT just output JSON - you must actually call the step_complete tool!"#;

    println!(
        "\n{} {}",
        "Query:".yellow().bold(),
        "Fix bug + call step_complete tool"
    );
    println!(
        "{}",
        "(Claude must call our custom MCP tool!)"
            .green()
            .dimmed()
    );
    client.query(query).await?;

    // 7. Receive and display response
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

    // 8. Disconnect Claude
    client.disconnect().await?;

    // ============================================================
    // 9. CHECK IF OUR CUSTOM TOOL WAS CALLED
    // ============================================================

    println!("\n{}", "Custom tool status:".blue().bold());
    println!("{}", "-".repeat(60).dimmed());

    if step_completed.load(Ordering::SeqCst) {
        println!(
            "  {} {}",
            "step_complete called:".green(),
            "YES".green().bold()
        );

        // Show the captured result
        if let Some(ref result) = *captured_result.lock().unwrap() {
            println!(
                "  {} {}",
                "Reported success:".dimmed(),
                if result.success {
                    "true".green()
                } else {
                    "false".red()
                }
            );
            println!("  {} {}", "Reported summary:".dimmed(), result.summary);
            if let Some(ref reason) = result.reason {
                println!("  {} {}", "Failure reason:".dimmed(), reason);
            }
        }
    } else {
        println!(
            "  {} {}",
            "step_complete called:".red(),
            "NO".red().bold()
        );
        println!(
            "  {}",
            "Claude did not call our custom tool!".yellow().dimmed()
        );
    }
    println!("{}", "-".repeat(60).dimmed());

    // ============================================================
    // 10. VERIFICATION - Run cargo test
    // ============================================================

    println!("\n{}", "Fixed code:".green().bold());
    println!("{}", "-".repeat(60).dimmed());
    let fixed_code = fs::read_to_string(&lib_path)?;
    println!("{}", fixed_code.trim());
    println!("{}", "-".repeat(60).dimmed());

    println!("\n{}", "Running verification...".cyan().bold());
    println!("{}", "$ cargo test".dimmed());
    println!("{}", "-".repeat(60).dimmed());

    let (passed, test_output) = run_cargo_test(temp.path())?;

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

    // Compare custom tool claim vs verification
    println!("\n{}", "Result comparison:".blue().bold());

    let claimed_success = captured_result
        .lock()
        .unwrap()
        .as_ref()
        .map(|r| r.success)
        .unwrap_or(false);

    println!(
        "  {} {}",
        "Tool says:".dimmed(),
        if step_completed.load(Ordering::SeqCst) {
            if claimed_success {
                "success".green()
            } else {
                "failed".red()
            }
        } else {
            "not called".yellow()
        }
    );
    println!(
        "  {} {}",
        "Tests say:".dimmed(),
        if passed {
            "passed".green()
        } else {
            "failed".red()
        }
    );

    if step_completed.load(Ordering::SeqCst) && claimed_success == passed {
        println!(
            "  {} {}",
            "Match:".green(),
            "Tool's claim matches verification!"
        );
    } else if !step_completed.load(Ordering::SeqCst) {
        println!(
            "  {} {}",
            "Note:".yellow(),
            "Tool was not called, cannot compare"
        );
    } else {
        println!(
            "  {} {}",
            "Mismatch:".red().bold(),
            "Tool's claim does NOT match verification!"
        );
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
    let edit_count = tool_uses.iter().filter(|t| *t == "Edit").count();
    let custom_count = tool_uses
        .iter()
        .filter(|t| t.contains("step_complete"))
        .count();
    println!("  {} {}", "Read:".dimmed(), read_count);
    println!("  {} {}", "Edit:".dimmed(), edit_count);
    println!(
        "  {} {} {}",
        "step_complete:".dimmed(),
        custom_count,
        "(custom MCP tool)".cyan().dimmed()
    );

    info!("{}", "Done.".green());
    println!(
        "\n{}",
        "Custom MCP tools enable powerful integrations!"
            .yellow()
            .dimmed()
    );
    println!(
        "{}",
        "Ideas: ask_human, log_assumption, propose_task, etc."
            .dimmed()
    );

    Ok(())
}
