//! Run a full session with DAG resolution against the two-bugs template.
//!
//! Requires ANTHROPIC_API_KEY environment variable.
//!
//! Usage: cargo run --example 04-session

use std::fs;

use rloop::config::Config;
use rloop::model::Model;
use rloop::session::run_session;
use rloop::task::TaskId;
use rloop::template::{Template, copy_template};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        eprintln!("Set ANTHROPIC_API_KEY to run this example.");
        std::process::exit(1);
    }

    let dir = copy_template(&Template::TwoBugs)?;
    let repo = dir.path().to_path_buf();

    // Two-task DAG: 00 fixes add, 01 fixes multiply (depends on 00)
    let tasks_dir = repo.join(".rloop").join("tasks");
    fs::create_dir_all(&tasks_dir)?;

    fs::write(
        tasks_dir.join("00.md"),
        "---\nid: 0\nmodel: Haiku\nverification: \"cargo test test_add\"\n---\nFix the `add` function in `src/lib.rs`. It returns the wrong result.\n",
    )?;
    fs::write(
        tasks_dir.join("01.md"),
        "---\nid: 1\nmodel: Haiku\ndepends_on: [0]\nverification: \"cargo test test_multiply\"\n---\nFix the `multiply` function in `src/lib.rs`. It returns the wrong result.\n",
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
