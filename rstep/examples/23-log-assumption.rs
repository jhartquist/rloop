//! Example 23: Human Queue - log_assumption Tool
//!
//! Demonstrates the `log_assumption` MCP tool for Claude to log decisions
//! and judgment calls for later human review.
//!
//! Key difference from ask_question:
//!   - ask_question is often BLOCKING - work stops until answered
//!   - log_assumption is usually NON-BLOCKING - Claude continues, human reviews later
//!
//! What this does:
//! 1. Creates an in-process MCP server with a `log_assumption` tool
//! 2. Gives Claude a task requiring judgment calls
//! 3. Claude logs assumptions as it makes decisions
//! 4. Assumptions are captured for human review
//! 5. Work continues (non-blocking by default)
//! 6. At the end, show all assumptions made
//!
//! Run with:
//!   cargo run --example 23-log-assumption
//!
//! Key additions from previous examples:
//!   - `log_assumption` tool for recording decisions
//!   - `Assumption` struct with decision, reasoning, alternatives
//!   - `needs_review` flag for assumptions requiring human approval

use anyhow::Result;
use claude_agent_sdk_rs::types::mcp::ToolResultContent as McpToolResultContent;
use claude_agent_sdk_rs::{
    create_sdk_mcp_server, tool, ClaudeAgentOptions, ClaudeClient, McpServerConfig, McpServers,
    Message, PermissionMode, ToolResult,
};
use colored::Colorize;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;
use tracing::{debug, info};

// ============================================================
// TYPES
// ============================================================

/// An assumption captured from Claude via the log_assumption tool
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Assumption {
    /// What Claude decided to do
    decision: String,
    /// Why Claude made this decision
    reasoning: String,
    /// Other options Claude considered
    #[serde(default)]
    alternatives: Vec<String>,
    /// If true, human must review before merge
    needs_review: bool,
}

/// Arguments for the log_assumption tool
#[derive(Debug, Deserialize)]
struct LogAssumptionArgs {
    decision: String,
    reasoning: String,
    #[serde(default)]
    alternatives: Option<Vec<String>>,
    #[serde(default)]
    needs_review: Option<bool>,
}

/// Blocker types that can prevent step completion
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Blocker {
    Question { question: String, context: Option<String> },
    Assumption { decision: String, needs_review: bool },
}

/// Result of a step execution
#[derive(Debug, Clone, Serialize, Deserialize)]
enum StepResult {
    Success { summary: String },
    Failed { reason: String },
    Blocked { blocker: Blocker },
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
        "|             HUMAN QUEUE: log_assumption TOOL             |"
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
        "|  Claude logs decisions for later human review!           |"
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

    // Show the code Claude will work with
    let lib_path = temp.path().join("src/lib.rs");
    println!("\n{}", "Current code:".yellow().bold());
    println!("{}", "-".repeat(60).dimmed());
    let original_code = fs::read_to_string(&lib_path)?;
    println!("{}", original_code.trim());
    println!("{}", "-".repeat(60).dimmed());

    // ============================================================
    // 3. CREATE CUSTOM MCP SERVER WITH log_assumption TOOL
    // ============================================================

    println!("\n{}", "Creating custom MCP server...".cyan().bold());

    // Storage for captured assumptions
    let captured_assumptions: Arc<Mutex<Vec<Assumption>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_assumptions_clone = captured_assumptions.clone();

    // Define our log_assumption tool
    let log_assumption_tool = tool!(
        "log_assumption",
        "Log an assumption you're making. Human will review later. Use when making judgment calls \
         about implementation details, design decisions, or anything that could reasonably be done \
         differently. Work continues after logging - this is non-blocking unless needs_review=true.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "decision": {
                    "type": "string",
                    "description": "What you decided to do"
                },
                "reasoning": {
                    "type": "string",
                    "description": "Why you made this decision"
                },
                "alternatives": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Other options you considered"
                },
                "needs_review": {
                    "type": "boolean",
                    "description": "If true, human must review before merge. Use for significant decisions."
                }
            },
            "required": ["decision", "reasoning"]
        }),
        move |args: serde_json::Value| {
            let assumptions_storage = captured_assumptions_clone.clone();
            async move {
                // Parse the arguments
                let parsed: LogAssumptionArgs = serde_json::from_value(args)?;

                let needs_review = parsed.needs_review.unwrap_or(false);
                let alternatives = parsed.alternatives.unwrap_or_default();

                let assumption = Assumption {
                    decision: parsed.decision.clone(),
                    reasoning: parsed.reasoning.clone(),
                    alternatives: alternatives.clone(),
                    needs_review,
                };

                println!("\n{}", "=".repeat(60).blue());
                println!(
                    "{}",
                    "  ASSUMPTION LOGGED".blue().bold()
                );
                println!("{}", "=".repeat(60).blue());
                println!("  {} {}", "Decision:".yellow(), parsed.decision);
                println!("  {} {}", "Reasoning:".dimmed(), parsed.reasoning);
                if !alternatives.is_empty() {
                    println!("  {}", "Alternatives considered:".dimmed());
                    for alt in &alternatives {
                        println!("    {} {}", "-".dimmed(), alt);
                    }
                }
                println!(
                    "  {} {}",
                    "Needs review:".dimmed(),
                    if needs_review {
                        "YES (must approve before merge)".yellow().to_string()
                    } else {
                        "No (informational)".green().to_string()
                    }
                );
                println!("{}", "=".repeat(60).blue());

                // Store the assumption
                assumptions_storage.lock().unwrap().push(assumption);

                // Return acknowledgment - work continues
                let response = if needs_review {
                    "Assumption logged for required review. Continue with your implementation."
                } else {
                    "Assumption logged for informational review. Continue with your implementation."
                };

                Ok(ToolResult {
                    content: vec![McpToolResultContent::Text {
                        text: response.to_string(),
                    }],
                    is_error: false,
                })
            }
        }
    );

    // Create the MCP server with our tool
    let mcp_server = create_sdk_mcp_server("rstep-hq", "0.1.0", vec![log_assumption_tool]);

    println!(
        "  {} {}",
        "Server:".dimmed(),
        "rstep-hq v0.1.0".white()
    );
    println!("  {} {}", "Tools:".dimmed(), "log_assumption".cyan());

    // Build the MCP servers map
    let mut mcp_servers_map: HashMap<String, McpServerConfig> = HashMap::new();
    mcp_servers_map.insert("rstep-hq".to_string(), McpServerConfig::Sdk(mcp_server));

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
            // They appear as "mcp__rstep-hq__log_assumption"
        ])
        .mcp_servers(McpServers::Dict(mcp_servers_map))
        .permission_mode(PermissionMode::BypassPermissions)
        .cwd(temp.path())
        .build();

    // 5. Create client and connect
    let mut client = ClaudeClient::new(options);
    client.connect().await?;

    // 6. Send a query that requires judgment calls
    let query = r#"This is a Rust project. Read src/lib.rs and fix the bug in the add function.
Also add appropriate error handling.

IMPORTANT: You have access to the `mcp__rstep-hq__log_assumption` tool.
- When you make a judgment call about HOW to implement something, log it as an assumption.
- Include what you decided, why, and what alternatives you considered.
- Set needs_review=true for significant decisions that could reasonably go another way.
- Set needs_review=false for minor implementation details.

Examples of assumptions to log:
- Choice of error handling strategy (panic vs Result vs Option)
- Whether to add input validation
- Naming conventions chosen
- Edge case handling decisions

After logging your assumptions, proceed to make the changes."#;

    println!(
        "\n{} {}",
        "Query:".yellow().bold(),
        "Fix bug and add error handling (requires judgment)"
    );
    println!(
        "{}",
        "(Claude should log assumptions as it decides how to proceed)"
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
    // 9. DISPLAY CAPTURED ASSUMPTIONS
    // ============================================================

    let assumptions = captured_assumptions.lock().unwrap().clone();

    println!("\n{}", "Captured Assumptions:".blue().bold());
    println!("{}", "=".repeat(60).dimmed());

    if assumptions.is_empty() {
        println!(
            "  {}",
            "No assumptions logged - Claude didn't document any judgment calls".yellow()
        );
    } else {
        let needs_review_count = assumptions.iter().filter(|a| a.needs_review).count();
        let informational_count = assumptions.len() - needs_review_count;

        println!(
            "  {} {} total ({} need review, {} informational)",
            "Count:".dimmed(),
            assumptions.len().to_string().cyan().bold(),
            needs_review_count.to_string().yellow(),
            informational_count.to_string().green()
        );
        println!();

        for (i, assumption) in assumptions.iter().enumerate() {
            let review_badge = if assumption.needs_review {
                "[NEEDS REVIEW]".yellow().bold().to_string()
            } else {
                "[INFO]".green().dimmed().to_string()
            };

            println!(
                "  {} {} {}",
                format!("Assumption {}:", i + 1).cyan().bold(),
                review_badge,
                ""
            );
            println!("    {} {}", "Decision:".white().bold(), assumption.decision);
            println!("    {} {}", "Reasoning:".dimmed(), assumption.reasoning);
            if !assumption.alternatives.is_empty() {
                println!("    {}", "Alternatives:".dimmed());
                for alt in &assumption.alternatives {
                    println!("      {} {}", "-".dimmed(), alt);
                }
            }
            println!();
        }
    }
    println!("{}", "=".repeat(60).dimmed());

    // ============================================================
    // 10. DETERMINE STEP RESULT
    // ============================================================

    // Check if any assumptions need review before merge
    let blocking_assumption = assumptions.iter().find(|a| a.needs_review);

    let step_result = if let Some(a) = blocking_assumption {
        // If any assumption needs review, the step is blocked
        StepResult::Blocked {
            blocker: Blocker::Assumption {
                decision: a.decision.clone(),
                needs_review: true,
            },
        }
    } else if assumptions.is_empty() {
        // No assumptions logged - could be success or failure depending on context
        StepResult::Success {
            summary: "Completed without logging assumptions".to_string(),
        }
    } else {
        // Informational assumptions only - step succeeds
        StepResult::Success {
            summary: format!(
                "Completed with {} informational assumption(s) logged",
                assumptions.len()
            ),
        }
    };

    // ============================================================
    // 11. DISPLAY STEP RESULT
    // ============================================================

    println!("\n{}", "Step Result:".blue().bold());
    println!("{}", "-".repeat(60).dimmed());

    match &step_result {
        StepResult::Success { summary } => {
            println!(
                "  {} {}",
                "Status:".dimmed(),
                "SUCCESS".green().bold()
            );
            println!("  {} {}", "Summary:".dimmed(), summary);
        }
        StepResult::Failed { reason } => {
            println!(
                "  {} {}",
                "Status:".dimmed(),
                "FAILED".red().bold()
            );
            println!("  {} {}", "Reason:".dimmed(), reason);
        }
        StepResult::Blocked { blocker } => {
            println!(
                "  {} {}",
                "Status:".dimmed(),
                "BLOCKED".yellow().bold()
            );
            match blocker {
                Blocker::Question { question, context } => {
                    println!("  {} Question", "Blocker type:".dimmed());
                    println!("  {} {}", "Question:".dimmed(), question);
                    if let Some(ctx) = context {
                        println!("  {} {}", "Context:".dimmed(), ctx);
                    }
                }
                Blocker::Assumption { decision, needs_review: _ } => {
                    println!("  {} Assumption needs review", "Blocker type:".dimmed());
                    println!("  {} {}", "Decision:".dimmed(), decision);
                }
            }
        }
    }
    println!("{}", "-".repeat(60).dimmed());

    // Print JSON representation
    println!("\n{}", "Step Result (JSON):".blue().bold());
    println!("{}", serde_json::to_string_pretty(&step_result)?);

    // 12. Print token summary
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
    let assumption_count = tool_uses
        .iter()
        .filter(|t| t.contains("log_assumption"))
        .count();
    println!("  {} {}", "Read:".dimmed(), read_count);
    println!("  {} {}", "Edit:".dimmed(), edit_count);
    println!(
        "  {} {} {}",
        "log_assumption:".dimmed(),
        assumption_count,
        "(custom MCP tool)".cyan().dimmed()
    );

    // Show final code if it was modified
    if edit_count > 0 {
        let final_code = fs::read_to_string(&lib_path)?;
        if final_code != original_code {
            println!("\n{}", "Final code:".yellow().bold());
            println!("{}", "-".repeat(60).dimmed());
            println!("{}", final_code.trim());
            println!("{}", "-".repeat(60).dimmed());
        }
    }

    info!("{}", "Done.".green());
    println!(
        "\n{}",
        "Assumptions enable transparent decision-making!"
            .yellow()
            .dimmed()
    );
    println!(
        "{}",
        "Claude documents judgment calls, humans review later."
            .dimmed()
    );

    Ok(())
}
