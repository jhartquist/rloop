//! Example 24: Human Queue - propose_task Tool
//!
//! Demonstrates the `propose_task` MCP tool for Claude to suggest follow-up tasks
//! discovered during work.
//!
//! Key difference from ask_question and log_assumption:
//!   - ask_question is for clarifying current task requirements
//!   - log_assumption is for documenting decisions made
//!   - propose_task is for suggesting NEW work discovered during the current task
//!
//! What this does:
//! 1. Creates an in-process MCP server with a `propose_task` tool
//! 2. Gives Claude a task where follow-up work is likely discovered
//! 3. Claude can propose new tasks as it works (non-blocking)
//! 4. Tasks are queued for human approval
//! 5. On approval, tasks would be created in br (but we just capture them here)
//! 6. At the end, show all proposed tasks with their priorities
//!
//! Run with:
//!   cargo run --example 24-propose-task
//!
//! Key additions from previous examples:
//!   - `propose_task` tool for suggesting follow-up work
//!   - `ProposedTask` struct with title, description, priority, related_files
//!   - Priority levels: 0=critical, 1=high, 2=medium, 3=low, 4=backlog

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

/// A proposed task captured from Claude via the propose_task tool
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProposedTask {
    /// Short task title
    title: String,
    /// What needs to be done and why
    description: String,
    /// Priority: 0=critical, 1=high, 2=medium, 3=low, 4=backlog
    priority: u8,
    /// Files related to this task
    #[serde(default)]
    related_files: Vec<String>,
}

impl ProposedTask {
    fn priority_label(&self) -> &'static str {
        match self.priority {
            0 => "CRITICAL",
            1 => "HIGH",
            2 => "MEDIUM",
            3 => "LOW",
            4 => "BACKLOG",
            _ => "UNKNOWN",
        }
    }
}

/// Arguments for the propose_task tool
#[derive(Debug, Deserialize)]
struct ProposeTaskArgs {
    title: String,
    description: String,
    #[serde(default)]
    priority: Option<u8>,
    #[serde(default)]
    related_files: Option<Vec<String>>,
}

/// Blocker types that can prevent step completion
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Blocker {
    Question { question: String, context: Option<String> },
    Assumption { decision: String, needs_review: bool },
    ProposedTask { title: String, priority: u8 },
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
        "|             HUMAN QUEUE: propose_task TOOL               |"
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
        "|  Claude proposes follow-up tasks discovered during work! |"
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
    // 3. CREATE CUSTOM MCP SERVER WITH propose_task TOOL
    // ============================================================

    println!("\n{}", "Creating custom MCP server...".cyan().bold());

    // Storage for captured proposed tasks
    let captured_tasks: Arc<Mutex<Vec<ProposedTask>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_tasks_clone = captured_tasks.clone();

    // Define our propose_task tool
    let propose_task_tool = tool!(
        "propose_task",
        "Propose a new task you discovered while working. Human will review and approve. \
         Use when you notice: missing tests, documentation gaps, potential improvements, \
         code smells, or related work that should be tracked. This is NON-BLOCKING - \
         continue with your current task after proposing.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Short task title (imperative form, e.g., 'Add unit tests for add function')"
                },
                "description": {
                    "type": "string",
                    "description": "What needs to be done and why"
                },
                "priority": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 4,
                    "description": "Suggested priority: 0=critical, 1=high, 2=medium, 3=low, 4=backlog"
                },
                "related_files": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Files related to this task"
                }
            },
            "required": ["title", "description"]
        }),
        move |args: serde_json::Value| {
            let tasks_storage = captured_tasks_clone.clone();
            async move {
                // Parse the arguments
                let parsed: ProposeTaskArgs = serde_json::from_value(args)?;

                // Default priority to MEDIUM (2) if not specified
                let priority = parsed.priority.unwrap_or(2).min(4);
                let related_files = parsed.related_files.unwrap_or_default();

                let task = ProposedTask {
                    title: parsed.title.clone(),
                    description: parsed.description.clone(),
                    priority,
                    related_files: related_files.clone(),
                };

                let priority_label = task.priority_label();
                let priority_color = match priority {
                    0 => "CRITICAL".red().bold(),
                    1 => "HIGH".yellow().bold(),
                    2 => "MEDIUM".cyan(),
                    3 => "LOW".green(),
                    _ => "BACKLOG".dimmed(),
                };

                println!("\n{}", "=".repeat(60).green());
                println!(
                    "{}",
                    "  TASK PROPOSED".green().bold()
                );
                println!("{}", "=".repeat(60).green());
                println!("  {} {}", "Title:".yellow(), parsed.title);
                println!("  {} {}", "Description:".dimmed(), parsed.description);
                println!(
                    "  {} {} ({})",
                    "Priority:".dimmed(),
                    priority_color,
                    priority_label
                );
                if !related_files.is_empty() {
                    println!("  {}", "Related files:".dimmed());
                    for file in &related_files {
                        println!("    {} {}", "-".dimmed(), file);
                    }
                }
                println!("{}", "=".repeat(60).green());

                // Store the proposed task
                tasks_storage.lock().unwrap().push(task);

                // Return acknowledgment - work continues (non-blocking)
                Ok(ToolResult {
                    content: vec![McpToolResultContent::Text {
                        text: format!(
                            "Task proposed: '{}' (priority: {}). Queued for human review. Continue with your current work.",
                            parsed.title, priority_label
                        ),
                    }],
                    is_error: false,
                })
            }
        }
    );

    // Create the MCP server with our tool
    let mcp_server = create_sdk_mcp_server("rstep-hq", "0.1.0", vec![propose_task_tool]);

    println!(
        "  {} {}",
        "Server:".dimmed(),
        "rstep-hq v0.1.0".white()
    );
    println!("  {} {}", "Tools:".dimmed(), "propose_task".cyan());

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
            // They appear as "mcp__rstep-hq__propose_task"
        ])
        .mcp_servers(McpServers::Dict(mcp_servers_map))
        .permission_mode(PermissionMode::BypassPermissions)
        .cwd(temp.path())
        .build();

    // 5. Create client and connect
    let mut client = ClaudeClient::new(options);
    client.connect().await?;

    // 6. Send a query - fix the bug, but also look for other issues
    let query = r#"This is a Rust project. Read src/lib.rs and fix the bug in the add function.

IMPORTANT: You have access to the `mcp__rstep-hq__propose_task` tool. You MUST use it!

BEFORE fixing the bug, analyze the code and use propose_task to suggest at least 2-3 improvements:
- The code only has one test - propose adding more comprehensive tests (edge cases, negative numbers, overflow)
- The documentation is minimal - propose adding better doc comments with examples
- This is a math library with only one function - propose adding subtract, multiply, divide functions
- Consider proposing any other improvements you notice

Set appropriate priorities:
- 0 = critical (blocking issues, security problems)
- 1 = high (important bugs, missing critical functionality)
- 2 = medium (improvements, enhancements)
- 3 = low (nice-to-have, minor improvements)
- 4 = backlog (future ideas, not urgent)

AFTER proposing the follow-up tasks, fix the bug in the add function."#;

    println!(
        "\n{} {}",
        "Query:".yellow().bold(),
        "Fix bug (and propose follow-up tasks discovered)"
    );
    println!(
        "{}",
        "(Claude should propose tasks for improvements it notices)"
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
    // 9. DISPLAY CAPTURED PROPOSED TASKS
    // ============================================================

    let proposed_tasks = captured_tasks.lock().unwrap().clone();

    println!("\n{}", "Proposed Tasks (for human review):".green().bold());
    println!("{}", "=".repeat(60).dimmed());

    if proposed_tasks.is_empty() {
        println!(
            "  {}",
            "No tasks proposed - Claude didn't discover any follow-up work".yellow()
        );
    } else {
        // Group by priority
        let critical: Vec<_> = proposed_tasks.iter().filter(|t| t.priority == 0).collect();
        let high: Vec<_> = proposed_tasks.iter().filter(|t| t.priority == 1).collect();
        let medium: Vec<_> = proposed_tasks.iter().filter(|t| t.priority == 2).collect();
        let low: Vec<_> = proposed_tasks.iter().filter(|t| t.priority == 3).collect();
        let backlog: Vec<_> = proposed_tasks.iter().filter(|t| t.priority == 4).collect();

        println!(
            "  {} {} total",
            "Count:".dimmed(),
            proposed_tasks.len().to_string().cyan().bold()
        );
        println!(
            "    {} critical, {} high, {} medium, {} low, {} backlog",
            critical.len().to_string().red(),
            high.len().to_string().yellow(),
            medium.len().to_string().cyan(),
            low.len().to_string().green(),
            backlog.len().to_string().dimmed()
        );
        println!();

        for (i, task) in proposed_tasks.iter().enumerate() {
            let priority_badge = match task.priority {
                0 => "[CRITICAL]".red().bold().to_string(),
                1 => "[HIGH]".yellow().bold().to_string(),
                2 => "[MEDIUM]".cyan().to_string(),
                3 => "[LOW]".green().to_string(),
                _ => "[BACKLOG]".dimmed().to_string(),
            };

            println!(
                "  {} {} {}",
                format!("Task {}:", i + 1).cyan().bold(),
                priority_badge,
                task.title.white().bold()
            );
            println!("    {} {}", "Description:".dimmed(), task.description);
            if !task.related_files.is_empty() {
                println!(
                    "    {} {}",
                    "Related:".dimmed(),
                    task.related_files.join(", ")
                );
            }
            println!();
        }
    }
    println!("{}", "=".repeat(60).dimmed());

    // ============================================================
    // 10. DETERMINE STEP RESULT
    // ============================================================

    // propose_task is non-blocking, so step always succeeds if the work was done
    // We just report how many tasks were proposed for follow-up
    let step_result = StepResult::Success {
        summary: format!(
            "Completed with {} follow-up task(s) proposed for review",
            proposed_tasks.len()
        ),
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
                Blocker::ProposedTask { title, priority } => {
                    println!("  {} Critical task proposed", "Blocker type:".dimmed());
                    println!("  {} {}", "Task:".dimmed(), title);
                    println!("  {} {}", "Priority:".dimmed(), priority);
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
    let propose_count = tool_uses
        .iter()
        .filter(|t| t.contains("propose_task"))
        .count();
    println!("  {} {}", "Read:".dimmed(), read_count);
    println!("  {} {}", "Edit:".dimmed(), edit_count);
    println!(
        "  {} {} {}",
        "propose_task:".dimmed(),
        propose_count,
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
        "propose_task enables proactive follow-up work discovery!"
            .yellow()
            .dimmed()
    );
    println!(
        "{}",
        "Claude notices improvements, humans approve, tasks get created."
            .dimmed()
    );

    Ok(())
}
