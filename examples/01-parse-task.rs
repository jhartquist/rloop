//! Parse and display task files.
//!
//! Usage: cargo run --example 01-parse-task

use std::fs;

use rloop::task::{load_all_tasks, load_task};

fn main() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let tasks_dir = dir.path();

    // Write sample task files
    fs::write(
        tasks_dir.join("00.md"),
        "---\nid: 0\n---\n# Setup project\n\nCreate directory structure.\n",
    )?;
    fs::write(
        tasks_dir.join("01.md"),
        "---\nid: 1\nmodel: Opus\ndepends_on: [0]\nverification: [\"cargo test\"]\ncompleted: false\n---\n# Implement feature\n\nBuild the core module.\n",
    )?;

    // Load a single task
    println!("=== Single task ===");
    let task = load_task(tasks_dir.join("01.md"))?;
    println!("  ID:           {}", task.id);
    println!("  Model:        {}", task.model);
    println!("  Depends on:   {:?}", task.depends_on);
    println!(
        "  Verification: {:?}",
        task.verification.as_ref().map(|v| v.as_ref())
    );
    println!("  Completed:    {}", task.completed);
    println!("  Description:");
    for line in task.description.lines().take(3) {
        println!("    {line}");
    }

    // Load all tasks
    println!("\n=== All tasks ===");
    let tasks = load_all_tasks(tasks_dir)?;
    for t in &tasks {
        let status = if t.completed { "done" } else { "todo" };
        println!(
            "  [{status}] {} — {}",
            t.id,
            t.description.lines().next().unwrap_or("")
        );
    }

    println!("\nLoaded {} task(s).", tasks.len());
    Ok(())
}
