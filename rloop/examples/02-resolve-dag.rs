//! Resolve a task DAG and display execution order.
//!
//! Usage: cargo run --example 02-resolve-dag

use std::fs;

use rloop::task::{TaskId, load_all_tasks, resolve_all_dag, resolve_dag};

fn main() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let tasks_dir = dir.path();

    // Diamond DAG: 0 -> {1, 2} -> 3
    fs::write(tasks_dir.join("00.md"), "---\nid: 0\n---\n# Root task\n")?;
    fs::write(
        tasks_dir.join("01.md"),
        "---\nid: 1\ndepends_on: [0]\n---\n# Branch A\n",
    )?;
    fs::write(
        tasks_dir.join("02.md"),
        "---\nid: 2\ndepends_on: [0]\n---\n# Branch B\n",
    )?;
    fs::write(
        tasks_dir.join("03.md"),
        "---\nid: 3\ndepends_on: [1, 2]\n---\n# Merge point\n",
    )?;

    let tasks = load_all_tasks(tasks_dir)?;

    // Resolve for target task 3
    println!("=== DAG for task 03 ===");
    let plan = resolve_dag(&tasks, TaskId(3))?;
    println!("  Target: {}", plan.target);
    print!("  Execution order:");
    for step in &plan.steps {
        print!(" {step}");
    }
    println!();

    // Resolve all tasks
    println!("\n=== All incomplete tasks ===");
    let all_plan = resolve_all_dag(&tasks)?;
    print!("  Execution order:");
    for step in &all_plan.steps {
        print!(" {step}");
    }
    println!();

    // Show a linear chain
    println!("\n=== Linear chain (task 01 only) ===");
    let linear = resolve_dag(&tasks, TaskId(1))?;
    print!("  Execution order:");
    for step in &linear.steps {
        print!(" {step}");
    }
    println!();

    println!("\nDone.");
    Ok(())
}
