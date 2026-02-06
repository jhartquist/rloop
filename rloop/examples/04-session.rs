//! Run a full session with DAG resolution.
//!
//! Requires ANTHROPIC_API_KEY environment variable.
//!
//! Usage: cargo run --example 04-session

use std::fs;
use std::process::Command;

use rloop::config::Config;
use rloop::model::Model;
use rloop::session::run_session;
use rloop::task::TaskId;

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
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        eprintln!("Set ANTHROPIC_API_KEY to run this example.");
        std::process::exit(1);
    }

    let dir = tempfile::tempdir()?;
    let repo = dir.path().to_path_buf();

    init_repo(&repo);
    let tasks_dir = repo.join(".rloop").join("tasks");
    fs::create_dir_all(&tasks_dir)?;

    // Two-task DAG: 00 -> 01
    fs::write(
        tasks_dir.join("00.md"),
        "---\nid: 0\nmodel: Haiku\n---\nCreate `src/lib.rs` with a function `pub fn greet() -> &'static str { \"hello\" }`.\n",
    )?;
    fs::write(
        tasks_dir.join("01.md"),
        "---\nid: 1\nmodel: Haiku\ndepends_on: [0]\n---\nAdd a test in `src/lib.rs` that asserts `greet()` returns \"hello\".\n",
    )?;

    let config = Config {
        model: Model::Haiku,
        max_turns: 10,
        max_retries: 2,
    };

    println!("Running session targeting task 01...");
    let result = run_session(&repo, TaskId(1), &config, false).await?;

    println!("\nSession: {}", result.session_id);
    println!("Branch: {}", result.branch);
    println!("Completed: {:?}", result.completed_tasks);
    if !result.failed_tasks.is_empty() {
        println!("Failed: {:?}", result.failed_tasks);
    }
    if !result.skipped_tasks.is_empty() {
        println!("Skipped: {:?}", result.skipped_tasks);
    }
    println!("Auto-merged: {}", result.auto_merged);

    Ok(())
}
