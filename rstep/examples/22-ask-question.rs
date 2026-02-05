//! Example 22: Human Queue - ask_question Tool
//!
//! Demonstrates the `ask_question` MCP tool for Claude to ask blocking questions
//! when requirements are unclear.
//!
//! What this does:
//! 1. Creates an in-process MCP server with an `ask_question` tool
//! 2. Gives Claude a task with intentionally unclear requirements
//! 3. Claude can call `ask_question` when it needs clarification
//! 4. Tool captures the question and signals the step is blocked
//! 5. Returns `StepResult::Blocked { blocker: Question { ... } }`
//!
//! Run with:
//!   cargo run --example 22-ask-question
//!
//! Key additions from previous examples:
//!   - `ask_question` tool for human-in-the-loop interactions
//!   - `Question` struct capturing question, context, and blocking status
//!   - `StepResult::Blocked` with `Blocker::Question` variant

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

/// A question captured from Claude via the ask_question tool
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Question {
    /// The question Claude wants to ask
    question: String,
    /// Optional context explaining why this question is needed
    #[serde(default)]
    context: Option<String>,
    /// If true, work should stop until this is answered
    blocking: bool,
}

/// Arguments for the ask_question tool
#[derive(Debug, Deserialize)]
struct AskQuestionArgs {
    question: String,
    #[serde(default)]
    context: Option<String>,
    blocking: bool,
}

/// Blocker types that can prevent step completion
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Blocker {
    Question(Question),
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
        "|              HUMAN QUEUE: ask_question TOOL              |"
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
        "|  Claude can ask clarifying questions when blocked!       |"
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
    // 3. CREATE CUSTOM MCP SERVER WITH ask_question TOOL
    // ============================================================

    println!("\n{}", "Creating custom MCP server...".cyan().bold());

    // Storage for captured questions
    let captured_questions: Arc<Mutex<Vec<Question>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_questions_clone = captured_questions.clone();

    // Define our ask_question tool
    let ask_question_tool = tool!(
        "ask_question",
        "Ask the human a clarifying question. Use when requirements are unclear or you need guidance. \
         If blocking=true, work should stop until the human answers. If blocking=false, you may continue \
         with your best guess while the question is pending.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the human"
                },
                "context": {
                    "type": "string",
                    "description": "Why you need this answered - what are you trying to decide?"
                },
                "blocking": {
                    "type": "boolean",
                    "description": "If true, stop work until answered. If false, continue with best guess."
                }
            },
            "required": ["question", "blocking"]
        }),
        move |args: serde_json::Value| {
            let questions_storage = captured_questions_clone.clone();
            async move {
                // Parse the arguments
                let parsed: AskQuestionArgs = serde_json::from_value(args)?;

                let question = Question {
                    question: parsed.question.clone(),
                    context: parsed.context.clone(),
                    blocking: parsed.blocking,
                };

                println!("\n{}", "=".repeat(60).magenta());
                println!(
                    "{}",
                    "  QUESTION FROM CLAUDE".magenta().bold()
                );
                println!("{}", "=".repeat(60).magenta());
                println!("  {} {}", "Question:".yellow(), parsed.question);
                if let Some(ref ctx) = parsed.context {
                    println!("  {} {}", "Context:".dimmed(), ctx);
                }
                println!(
                    "  {} {}",
                    "Blocking:".dimmed(),
                    if parsed.blocking {
                        "YES (work will stop)".red().to_string()
                    } else {
                        "NO (will continue with best guess)".green().to_string()
                    }
                );
                println!("{}", "=".repeat(60).magenta());

                // Store the question
                questions_storage.lock().unwrap().push(question);

                // Return acknowledgment
                let response = if parsed.blocking {
                    "Question recorded. Work is BLOCKED until human responds. Do not proceed with changes."
                } else {
                    "Question recorded. You may continue with your best judgment while awaiting response."
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
    let mcp_server = create_sdk_mcp_server("rstep-hq", "0.1.0", vec![ask_question_tool]);

    println!(
        "  {} {}",
        "Server:".dimmed(),
        "rstep-hq v0.1.0".white()
    );
    println!("  {} {}", "Tools:".dimmed(), "ask_question".cyan());

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
            // They appear as "mcp__rstep-hq__ask_question"
        ])
        .mcp_servers(McpServers::Dict(mcp_servers_map))
        .permission_mode(PermissionMode::BypassPermissions)
        .cwd(temp.path())
        .build();

    // 5. Create client and connect
    let mut client = ClaudeClient::new(options);
    client.connect().await?;

    // 6. Send our AMBIGUOUS query - Claude should ask for clarification
    let query = r#"This is a Rust project. Read src/lib.rs and improve the add function.

IMPORTANT: You have access to the `mcp__rstep-hq__ask_question` tool.
- If the requirements are unclear, call `ask_question` with blocking=true to get clarification BEFORE making changes.
- Do NOT guess at what "improve" means - ask the human what specific improvement they want.

Examples of things you might ask:
- What kind of improvement is needed? (performance, error handling, documentation, etc.)
- Are there specific edge cases to handle?
- Should I add input validation?

Be thoughtful - don't just ask generic questions. Look at the code and ask specific, relevant questions."#;

    println!(
        "\n{} {}",
        "Query:".yellow().bold(),
        "Improve the add function (ambiguous!)"
    );
    println!(
        "{}",
        "(Claude should ask for clarification)"
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
    // 9. DETERMINE STEP RESULT BASED ON CAPTURED QUESTIONS
    // ============================================================

    let questions = captured_questions.lock().unwrap().clone();

    println!("\n{}", "Captured Questions:".blue().bold());
    println!("{}", "-".repeat(60).dimmed());

    if questions.is_empty() {
        println!(
            "  {}",
            "No questions asked - Claude proceeded without clarification".yellow()
        );
    } else {
        for (i, q) in questions.iter().enumerate() {
            println!(
                "  {} {}",
                format!("Q{}:", i + 1).cyan().bold(),
                q.question
            );
            if let Some(ref ctx) = q.context {
                println!("     {} {}", "Context:".dimmed(), ctx);
            }
            println!(
                "     {} {}",
                "Blocking:".dimmed(),
                if q.blocking { "Yes".red() } else { "No".green() }
            );
        }
    }
    println!("{}", "-".repeat(60).dimmed());

    // Determine the step result
    let step_result = if let Some(blocking_q) = questions.iter().find(|q| q.blocking) {
        StepResult::Blocked {
            blocker: Blocker::Question(blocking_q.clone()),
        }
    } else if questions.is_empty() {
        // Claude didn't ask any questions
        StepResult::Failed {
            reason: "Claude did not ask for clarification on ambiguous requirements".to_string(),
        }
    } else {
        // Non-blocking questions asked, Claude continued
        StepResult::Success {
            summary: format!(
                "Completed with {} non-blocking question(s) pending",
                questions.len()
            ),
        }
    };

    // ============================================================
    // 10. DISPLAY STEP RESULT
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
                Blocker::Question(q) => {
                    println!("  {} Question", "Blocker type:".dimmed());
                    println!("  {} {}", "Question:".dimmed(), q.question);
                    if let Some(ref ctx) = q.context {
                        println!("  {} {}", "Context:".dimmed(), ctx);
                    }
                }
            }
        }
    }
    println!("{}", "-".repeat(60).dimmed());

    // Print JSON representation
    println!("\n{}", "Step Result (JSON):".blue().bold());
    println!("{}", serde_json::to_string_pretty(&step_result)?);

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
    let ask_count = tool_uses
        .iter()
        .filter(|t| t.contains("ask_question"))
        .count();
    println!("  {} {}", "Read:".dimmed(), read_count);
    println!(
        "  {} {} {}",
        "ask_question:".dimmed(),
        ask_count,
        "(custom MCP tool)".cyan().dimmed()
    );

    info!("{}", "Done.".green());
    println!(
        "\n{}",
        "Human queue enables async human-in-the-loop workflows!"
            .yellow()
            .dimmed()
    );
    println!(
        "{}",
        "Questions get queued, humans review in batches, agents stay unblocked."
            .dimmed()
    );

    Ok(())
}
