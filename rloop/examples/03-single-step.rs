//! Run a single step with a real Claude agent.
//!
//! Requires ANTHROPIC_API_KEY environment variable.
//!
//! Usage: cargo run --example 03-single-step

use std::fs;
use std::process::Command;

use rloop::config::Config;
use rloop::git::{cleanup_worktree, create_worktree};
use rloop::model::Model;
use rloop::step::{execute_step, StepOutcome};
use rloop::task::{load_task, TaskId};

fn init_repo(dir: &std::path::Path) {
    let run = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git failed");
    };
    run(&["init"]);
    run(&["config", "user.email", "test@test.com"]);
    run(&["config", "user.name", "Test"]);
    fs::write(dir.join("README.md"), "# test\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-m", "init"]);
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Check for API key
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        eprintln!("Set ANTHROPIC_API_KEY to run this example.");
        std::process::exit(1);
    }

    let dir = tempfile::tempdir()?;
    let repo = dir.path().to_path_buf();

    // Set up a git repo with a task
    init_repo(&repo);
    let tasks_dir = repo.join(".rloop").join("tasks");
    fs::create_dir_all(&tasks_dir)?;
    fs::write(
        tasks_dir.join("00.md"),
        "---\nid: 0\nmodel: Haiku\n---\nCreate a file called `hello.txt` containing \"Hello from rloop!\".\n",
    )?;

    let task = load_task(&tasks_dir.join("00.md"))?;
    let config = Config {
        model: Model::Haiku,
        max_turns: 10,
        max_retries: 1,
    };

    // Create worktree
    let worktree = create_worktree(&repo, TaskId(0), "main")?;
    println!("Worktree: {}", worktree.path.display());

    // Execute step
    println!("Running step...");
    let result = execute_step(&task, &worktree.path, &config, &[]).await?;

    match &result.outcome {
        StepOutcome::Success { summary } => {
            println!("Success: {summary}");
        }
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
