//! Run a single step with a real Claude agent against the bug-adder template.
//!
//! Usage: cargo run --example 03-single-step

use std::fs;

use rloop::config::Config;
use rloop::git::{cleanup_worktree, create_worktree};
use rloop::model::Model;
use rloop::step::{StepOutcome, execute_step};
use rloop::task::{TaskId, load_task};
use rloop::template::{Template, copy_template};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let dir = copy_template(&Template::BugAdder)?;
    let repo = dir.path().to_path_buf();

    // Set up rloop task
    let tasks_dir = repo.join(".rloop").join("tasks");
    fs::create_dir_all(&tasks_dir)?;
    fs::write(
        tasks_dir.join("00.md"),
        "---\nid: 0\nmodel: Haiku\nverification: \"cargo test\"\n---\nFix the bug in `src/lib.rs`. The `add` function returns the wrong result.\n",
    )?;

    let task = load_task(tasks_dir.join("00.md"))?;
    let config = Config {
        model: Model::Haiku,
        max_turns: 10,
        max_retries: 1,
        timeout_seconds: 300,
        ..Config::default()
    };

    // Create worktree
    let worktree = create_worktree(&repo, TaskId(0), "main")?;
    println!("Worktree: {}", worktree.path.display());

    // Execute step
    println!("Running step...");
    let result = execute_step(&task, &worktree.path, &config, &[], 1).await?;

    match &result.outcome {
        StepOutcome::Success { summary } => println!("Success: {summary}"),
        StepOutcome::Failed { reason, learnings } => {
            println!("Failed: {reason}");
            println!("Learnings: {learnings}");
        }
    }

    println!("Model: {}", result.model);
    println!("Cost: ${:.4}", result.cost_usd);
    println!("Duration: {:.1}s", result.duration.as_secs_f64());
    if let Some(sid) = &result.cc_session_id {
        println!("CC Session: {sid}");
    }

    // Cleanup
    cleanup_worktree(&repo, &worktree)?;
    println!("Cleaned up.");

    Ok(())
}
