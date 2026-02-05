//! Example 25: Combined Human Queue Tools
//!
//! Demonstrates all three human queue tools working together:
//!   - `ask_question` - Ask blocking questions when requirements unclear
//!   - `log_assumption` - Log decisions and judgment calls for review
//!   - `propose_task` - Suggest follow-up work discovered during task
//!
//! This example gives Claude a complex, ambiguous task that might trigger
//! all three tools, then collects and displays items organized for batch review.
//!
//! Run with:
//!   cargo run --example 25-combined-human-queue
//!
//! Key additions from previous examples:
//!   - All three human queue tools in one MCP server
//!   - HumanQueue struct to collect all items
//!   - Organized output: Questions first, then Assumptions, then Proposed Tasks
//!   - Clear indication of which items need review vs informational

use anyhow::Result;
use claude_agent_sdk_rs::types::mcp::ToolResultContent as McpToolResultContent;
use claude_agent_sdk_rs::{
    create_sdk_mcp_server, tool, ClaudeAgentOptions, ClaudeClient, McpServerConfig, McpServers,
    Message, PermissionMode, SdkMcpTool, ToolResult,
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

/// Collection of all human queue items
#[derive(Debug, Clone, Default)]
struct HumanQueue {
    questions: Vec<Question>,
    assumptions: Vec<Assumption>,
    proposed_tasks: Vec<ProposedTask>,
}

impl HumanQueue {
    fn new() -> Self {
        Self::default()
    }

    fn add_question(&mut self, q: Question) {
        self.questions.push(q);
    }

    fn add_assumption(&mut self, a: Assumption) {
        self.assumptions.push(a);
    }

    fn add_proposed_task(&mut self, t: ProposedTask) {
        self.proposed_tasks.push(t);
    }

    fn is_empty(&self) -> bool {
        self.questions.is_empty() && self.assumptions.is_empty() && self.proposed_tasks.is_empty()
    }

    /// Returns true if there are any blocking items that need human action
    fn has_blocking_items(&self) -> bool {
        self.questions.iter().any(|q| q.blocking)
            || self.assumptions.iter().any(|a| a.needs_review)
    }

    fn total_items(&self) -> usize {
        self.questions.len() + self.assumptions.len() + self.proposed_tasks.len()
    }
}

/// Arguments for the ask_question tool
#[derive(Debug, Deserialize)]
struct AskQuestionArgs {
    question: String,
    #[serde(default)]
    context: Option<String>,
    blocking: bool,
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
    Assumption { decision: String },
}

/// Result of a step execution
#[derive(Debug, Clone, Serialize, Deserialize)]
enum StepResult {
    Success { summary: String },
    Failed { reason: String },
    Blocked { blocker: Blocker },
}

// ============================================================
// TOOL CREATION
// ============================================================

/// Create all three human queue tools with a shared queue
fn create_human_queue_tools(queue: Arc<Mutex<HumanQueue>>) -> Vec<SdkMcpTool> {
    let queue_for_question = queue.clone();
    let queue_for_assumption = queue.clone();
    let queue_for_task = queue.clone();

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
            let storage = queue_for_question.clone();
            async move {
                let parsed: AskQuestionArgs = serde_json::from_value(args)?;

                let question = Question {
                    question: parsed.question.clone(),
                    context: parsed.context.clone(),
                    blocking: parsed.blocking,
                };

                println!("\n{}", "=".repeat(60).magenta());
                println!("{}", "  QUESTION FROM CLAUDE".magenta().bold());
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

                storage.lock().unwrap().add_question(question);

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
            let storage = queue_for_assumption.clone();
            async move {
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
                println!("{}", "  ASSUMPTION LOGGED".blue().bold());
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

                storage.lock().unwrap().add_assumption(assumption);

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
            let storage = queue_for_task.clone();
            async move {
                let parsed: ProposeTaskArgs = serde_json::from_value(args)?;

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
                println!("{}", "  TASK PROPOSED".green().bold());
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

                storage.lock().unwrap().add_proposed_task(task);

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

    vec![ask_question_tool, log_assumption_tool, propose_task_tool]
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

/// Display all human queue items organized for batch review
fn display_human_queue(queue: &HumanQueue) {
    println!("\n{}", "=".repeat(70).cyan().bold());
    println!(
        "{}",
        "                    HUMAN QUEUE - BATCH REVIEW                    "
            .cyan()
            .bold()
    );
    println!("{}", "=".repeat(70).cyan().bold());

    if queue.is_empty() {
        println!(
            "\n  {}",
            "No items in queue - Claude didn't log any questions, assumptions, or tasks"
                .yellow()
        );
        println!("{}", "=".repeat(70).cyan().bold());
        return;
    }

    println!(
        "\n  {} {} total items ({} questions, {} assumptions, {} proposed tasks)",
        "Summary:".white().bold(),
        queue.total_items().to_string().cyan().bold(),
        queue.questions.len().to_string().magenta(),
        queue.assumptions.len().to_string().blue(),
        queue.proposed_tasks.len().to_string().green()
    );

    let blocking_questions = queue.questions.iter().filter(|q| q.blocking).count();
    let needs_review_assumptions = queue.assumptions.iter().filter(|a| a.needs_review).count();

    if queue.has_blocking_items() {
        println!(
            "  {} {} blocking questions, {} assumptions needing review",
            "Action required:".red().bold(),
            blocking_questions.to_string().red(),
            needs_review_assumptions.to_string().yellow()
        );
    } else {
        println!(
            "  {}",
            "No blocking items - all informational".green()
        );
    }

    // ==================== QUESTIONS ====================
    if !queue.questions.is_empty() {
        println!("\n{}", "-".repeat(70).magenta());
        println!(
            "  {} ({})",
            "QUESTIONS".magenta().bold(),
            queue.questions.len()
        );
        println!("{}", "-".repeat(70).magenta());

        for (i, q) in queue.questions.iter().enumerate() {
            let status = if q.blocking {
                "[BLOCKING]".red().bold().to_string()
            } else {
                "[pending]".dimmed().to_string()
            };

            println!(
                "\n  {} {} {}",
                format!("Q{}:", i + 1).magenta().bold(),
                status,
                ""
            );
            println!("    {} {}", "Question:".white().bold(), q.question);
            if let Some(ref ctx) = q.context {
                println!("    {} {}", "Context:".dimmed(), ctx);
            }
        }
    }

    // ==================== ASSUMPTIONS ====================
    if !queue.assumptions.is_empty() {
        println!("\n{}", "-".repeat(70).blue());
        println!(
            "  {} ({})",
            "ASSUMPTIONS".blue().bold(),
            queue.assumptions.len()
        );
        println!("{}", "-".repeat(70).blue());

        for (i, a) in queue.assumptions.iter().enumerate() {
            let status = if a.needs_review {
                "[NEEDS REVIEW]".yellow().bold().to_string()
            } else {
                "[info]".green().dimmed().to_string()
            };

            println!(
                "\n  {} {} {}",
                format!("A{}:", i + 1).blue().bold(),
                status,
                ""
            );
            println!("    {} {}", "Decision:".white().bold(), a.decision);
            println!("    {} {}", "Reasoning:".dimmed(), a.reasoning);
            if !a.alternatives.is_empty() {
                println!("    {}", "Alternatives:".dimmed());
                for alt in &a.alternatives {
                    println!("      {} {}", "-".dimmed(), alt);
                }
            }
        }
    }

    // ==================== PROPOSED TASKS ====================
    if !queue.proposed_tasks.is_empty() {
        println!("\n{}", "-".repeat(70).green());
        println!(
            "  {} ({})",
            "PROPOSED TASKS".green().bold(),
            queue.proposed_tasks.len()
        );
        println!("{}", "-".repeat(70).green());

        // Sort by priority for display
        let mut sorted_tasks = queue.proposed_tasks.clone();
        sorted_tasks.sort_by_key(|t| t.priority);

        for (i, t) in sorted_tasks.iter().enumerate() {
            let priority_badge = match t.priority {
                0 => "[CRITICAL]".red().bold().to_string(),
                1 => "[HIGH]".yellow().bold().to_string(),
                2 => "[MEDIUM]".cyan().to_string(),
                3 => "[LOW]".green().to_string(),
                _ => "[BACKLOG]".dimmed().to_string(),
            };

            println!(
                "\n  {} {} {}",
                format!("T{}:", i + 1).green().bold(),
                priority_badge,
                t.title.white().bold()
            );
            println!("    {} {}", "Description:".dimmed(), t.description);
            if !t.related_files.is_empty() {
                println!(
                    "    {} {}",
                    "Related:".dimmed(),
                    t.related_files.join(", ")
                );
            }
        }
    }

    println!("\n{}", "=".repeat(70).cyan().bold());
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
        "+------------------------------------------------------------------+"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|              COMBINED HUMAN QUEUE TOOLS                          |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|                                                                  |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|  ask_question + log_assumption + propose_task in one session!   |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "+------------------------------------------------------------------+"
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
    // 3. CREATE CUSTOM MCP SERVER WITH ALL THREE TOOLS
    // ============================================================

    println!("\n{}", "Creating custom MCP server...".cyan().bold());

    // Shared storage for all human queue items
    let human_queue: Arc<Mutex<HumanQueue>> = Arc::new(Mutex::new(HumanQueue::new()));

    // Create all three tools with shared storage
    let tools = create_human_queue_tools(human_queue.clone());

    // Create the MCP server with all tools
    let mcp_server = create_sdk_mcp_server("rstep-hq", "0.1.0", tools);

    println!(
        "  {} {}",
        "Server:".dimmed(),
        "rstep-hq v0.1.0".white()
    );
    println!(
        "  {} {} {} {}",
        "Tools:".dimmed(),
        "ask_question".magenta(),
        "log_assumption".blue(),
        "propose_task".green()
    );

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
        ])
        .mcp_servers(McpServers::Dict(mcp_servers_map))
        .permission_mode(PermissionMode::BypassPermissions)
        .cwd(temp.path())
        .build();

    // 5. Create client and connect
    let mut client = ClaudeClient::new(options);
    client.connect().await?;

    // 6. Send a complex, ambiguous query that might trigger all three tools
    let query = r#"This is a Rust project. Read src/lib.rs and review the code.

Your task: "Review this code and make improvements."

This is intentionally vague! You have access to three human queue tools. Use them appropriately:

1. `mcp__rstep-hq__ask_question` - The task says "make improvements" but doesn't specify what kind.
   Ask a BLOCKING question about what type of improvements are wanted (performance? error handling?
   documentation? all of the above?). Do NOT proceed with major changes until you ask.

2. `mcp__rstep-hq__log_assumption` - After reading the code, if you notice the obvious bug
   (a-b instead of a+b), log an assumption that fixing this bug is within scope of "improvements"
   even though the task didn't explicitly mention it. Set needs_review=true for this assumption.

3. `mcp__rstep-hq__propose_task` - While reviewing, propose follow-up tasks for things you notice:
   - The test coverage is minimal (only one test case)
   - Documentation could be improved
   - Other functions could be added (subtract, multiply, etc.)

   Propose at least 2 follow-up tasks with appropriate priorities.

IMPORTANT: Use ALL THREE tools appropriately:
- Ask the blocking question FIRST (before making changes)
- Log an assumption about the bug fix
- Propose follow-up tasks for improvements you notice

After using the tools, summarize what you found and what questions/assumptions/tasks you logged."#;

    println!(
        "\n{} {}",
        "Query:".yellow().bold(),
        "Review and improve the code (ambiguous task!)"
    );
    println!(
        "{}",
        "(Claude should use all three human queue tools)"
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
    // 9. DISPLAY COMBINED HUMAN QUEUE FOR BATCH REVIEW
    // ============================================================

    let queue = human_queue.lock().unwrap().clone();
    display_human_queue(&queue);

    // ============================================================
    // 10. DETERMINE STEP RESULT
    // ============================================================

    let step_result = if let Some(blocking_q) = queue.questions.iter().find(|q| q.blocking) {
        // Blocking question takes priority
        StepResult::Blocked {
            blocker: Blocker::Question {
                question: blocking_q.question.clone(),
                context: blocking_q.context.clone(),
            },
        }
    } else if let Some(needs_review_a) = queue.assumptions.iter().find(|a| a.needs_review) {
        // Then assumptions needing review
        StepResult::Blocked {
            blocker: Blocker::Assumption {
                decision: needs_review_a.decision.clone(),
            },
        }
    } else if queue.is_empty() {
        StepResult::Failed {
            reason: "Claude didn't use any human queue tools despite ambiguous task".to_string(),
        }
    } else {
        StepResult::Success {
            summary: format!(
                "Completed with {} queue items (all informational)",
                queue.total_items()
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
                Blocker::Assumption { decision } => {
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

    // Show tool summary
    println!("\n{}", "Tool summary:".blue().bold());
    let read_count = tool_uses.iter().filter(|t| *t == "Read").count();
    let edit_count = tool_uses.iter().filter(|t| *t == "Edit").count();
    let question_count = tool_uses.iter().filter(|t| t.contains("ask_question")).count();
    let assumption_count = tool_uses.iter().filter(|t| t.contains("log_assumption")).count();
    let propose_count = tool_uses.iter().filter(|t| t.contains("propose_task")).count();

    println!("  {} {}", "Read:".dimmed(), read_count);
    println!("  {} {}", "Edit:".dimmed(), edit_count);
    println!(
        "  {} {} {}",
        "ask_question:".dimmed(),
        question_count,
        "(human queue)".magenta().dimmed()
    );
    println!(
        "  {} {} {}",
        "log_assumption:".dimmed(),
        assumption_count,
        "(human queue)".blue().dimmed()
    );
    println!(
        "  {} {} {}",
        "propose_task:".dimmed(),
        propose_count,
        "(human queue)".green().dimmed()
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
        "Questions, assumptions, and tasks get queued for batch review."
            .dimmed()
    );

    Ok(())
}
