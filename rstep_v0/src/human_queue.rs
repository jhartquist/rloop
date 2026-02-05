//! Human queue tools for async human-in-the-loop workflows.
//!
//! This module provides three MCP tools that allow Claude to communicate
//! with humans asynchronously:
//!
//! - [`ask_question`] - Ask blocking questions when requirements are unclear
//! - [`log_assumption`] - Log decisions and judgment calls for review
//! - [`propose_task`] - Suggest follow-up work discovered during task
//!
//! All items are collected in a [`HumanQueue`] for batch review.
//!
//! # Example
//!
//! ```ignore
//! use rstep::human_queue::{HumanQueue, create_human_queue_tools};
//! use std::sync::{Arc, Mutex};
//!
//! let queue = Arc::new(Mutex::new(HumanQueue::new()));
//! let tools = create_human_queue_tools(queue.clone());
//!
//! // Register tools with MCP server...
//!
//! // After Claude uses the tools:
//! let queue = queue.lock().unwrap();
//! if queue.has_blocking_items() {
//!     println!("Step blocked - need human input");
//! }
//! ```

use claude_agent_sdk_rs::types::mcp::ToolResultContent as McpToolResultContent;
use claude_agent_sdk_rs::{tool, SdkMcpTool, ToolResult};
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

// ============================================================
// TYPES
// ============================================================

/// A question captured from Claude via the ask_question tool.
///
/// Questions can be blocking (work stops until answered) or non-blocking
/// (Claude continues with best guess while question is pending).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    /// The question Claude wants to ask
    pub question: String,
    /// Optional context explaining why this question is needed
    #[serde(default)]
    pub context: Option<String>,
    /// If true, work should stop until this is answered
    pub blocking: bool,
}

/// An assumption captured from Claude via the log_assumption tool.
///
/// Assumptions document judgment calls made during task execution.
/// They can require review (blocking merge) or be informational.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assumption {
    /// What Claude decided to do
    pub decision: String,
    /// Why Claude made this decision
    pub reasoning: String,
    /// Other options Claude considered
    #[serde(default)]
    pub alternatives: Vec<String>,
    /// If true, human must review before merge
    pub needs_review: bool,
}

/// A proposed task captured from Claude via the propose_task tool.
///
/// Proposed tasks are follow-up work discovered during the current task.
/// They are queued for human approval before being created.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposedTask {
    /// Short task title (imperative form)
    pub title: String,
    /// What needs to be done and why
    pub description: String,
    /// Priority: 0=critical, 1=high, 2=medium, 3=low, 4=backlog
    pub priority: u8,
    /// Files related to this task
    #[serde(default)]
    pub related_files: Vec<String>,
}

impl ProposedTask {
    /// Get a human-readable label for the priority level.
    pub fn priority_label(&self) -> &'static str {
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

/// Collection of all human queue items.
///
/// This struct collects questions, assumptions, and proposed tasks
/// from a Claude session for batch review by humans.
#[derive(Debug, Clone, Default)]
pub struct HumanQueue {
    /// Questions asked by Claude
    pub questions: Vec<Question>,
    /// Assumptions logged by Claude
    pub assumptions: Vec<Assumption>,
    /// Tasks proposed by Claude
    pub proposed_tasks: Vec<ProposedTask>,
}

impl HumanQueue {
    /// Create a new empty human queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a question to the queue.
    pub fn add_question(&mut self, q: Question) {
        self.questions.push(q);
    }

    /// Add an assumption to the queue.
    pub fn add_assumption(&mut self, a: Assumption) {
        self.assumptions.push(a);
    }

    /// Add a proposed task to the queue.
    pub fn add_proposed_task(&mut self, t: ProposedTask) {
        self.proposed_tasks.push(t);
    }

    /// Returns true if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.questions.is_empty() && self.assumptions.is_empty() && self.proposed_tasks.is_empty()
    }

    /// Returns true if there are any blocking items that need human action.
    ///
    /// Blocking items include:
    /// - Questions with `blocking = true`
    /// - Assumptions with `needs_review = true`
    pub fn has_blocking_items(&self) -> bool {
        self.questions.iter().any(|q| q.blocking)
            || self.assumptions.iter().any(|a| a.needs_review)
    }

    /// Returns the total number of items in the queue.
    pub fn total_items(&self) -> usize {
        self.questions.len() + self.assumptions.len() + self.proposed_tasks.len()
    }

    /// Returns the number of blocking questions.
    pub fn blocking_question_count(&self) -> usize {
        self.questions.iter().filter(|q| q.blocking).count()
    }

    /// Returns the number of assumptions needing review.
    pub fn needs_review_count(&self) -> usize {
        self.assumptions.iter().filter(|a| a.needs_review).count()
    }
}

// ============================================================
// TOOL ARGUMENT TYPES
// ============================================================

/// Arguments for the ask_question tool.
#[derive(Debug, Deserialize)]
struct AskQuestionArgs {
    question: String,
    #[serde(default)]
    context: Option<String>,
    blocking: bool,
}

/// Arguments for the log_assumption tool.
#[derive(Debug, Deserialize)]
struct LogAssumptionArgs {
    decision: String,
    reasoning: String,
    #[serde(default)]
    alternatives: Option<Vec<String>>,
    #[serde(default)]
    needs_review: Option<bool>,
}

/// Arguments for the propose_task tool.
#[derive(Debug, Deserialize)]
struct ProposeTaskArgs {
    title: String,
    description: String,
    #[serde(default)]
    priority: Option<u8>,
    #[serde(default)]
    related_files: Option<Vec<String>>,
}

// ============================================================
// TOOL CREATION
// ============================================================

/// Create all three human queue MCP tools with a shared queue.
///
/// Returns a vector of tools that can be registered with an MCP server:
/// - `ask_question` - For clarifying questions
/// - `log_assumption` - For documenting decisions
/// - `propose_task` - For suggesting follow-up work
///
/// # Arguments
///
/// * `queue` - Shared queue to collect items from all tools
///
/// # Example
///
/// ```ignore
/// use rstep::human_queue::{HumanQueue, create_human_queue_tools};
/// use std::sync::{Arc, Mutex};
///
/// let queue = Arc::new(Mutex::new(HumanQueue::new()));
/// let tools = create_human_queue_tools(queue.clone());
///
/// let mcp_server = create_sdk_mcp_server("rstep-hq", "0.1.0", tools);
/// ```
pub fn create_human_queue_tools(queue: Arc<Mutex<HumanQueue>>) -> Vec<SdkMcpTool> {
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
