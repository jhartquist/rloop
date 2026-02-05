//! rstep - Single-step task executor
//!
//! Run one step, get a result.
//!
//! # Modules
//!
//! - [`git`] - Git worktree operations for task isolation
//! - [`human_queue`] - Human-in-the-loop tools (ask_question, log_assumption, propose_task)
//! - [`step`] - Core step execution logic
//!
//! # Example
//!
//! ```ignore
//! use rstep::step::{run_step, load_config, Task, StepResult};
//!
//! let config = load_config(&repo_path);
//! let task = Task { id: "bd-123".into(), title: "Fix bug".into(), .. };
//! let result = run_step(&repo_path, &task, &config).await?;
//!
//! match result {
//!     StepResult::Success { summary, .. } => println!("Done: {}", summary),
//!     StepResult::Failed { reason, .. } => println!("Failed: {}", reason),
//!     StepResult::Blocked { blocker } => println!("Blocked: {:?}", blocker),
//! }
//! ```

pub mod git;
pub mod human_queue;
pub mod step;
