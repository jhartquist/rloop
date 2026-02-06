use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "rloop", about = "DAG-based task executor")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize .rloop/ directory in the current repo
    Init,
    /// Run a task (by ID) or all incomplete tasks
    Run {
        /// Task ID to run (e.g., 5 for task 05)
        id: Option<u32>,
        /// Run all incomplete tasks
        #[arg(long)]
        all: bool,
        /// Auto-merge session branch on success
        #[arg(long)]
        auto_merge: bool,
    },
    /// List tasks and their status
    List,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Init => cmd_init(),
        Command::Run { id, all, auto_merge } => cmd_run(id, all, auto_merge).await,
        Command::List => cmd_list(),
    }
}

fn cmd_init() -> anyhow::Result<()> {
    let repo = std::env::current_dir()?;
    let rloop_dir = rloop::config::init_rloop_dir(&repo)?;
    eprintln!("Initialized {}", rloop_dir.display());
    Ok(())
}

async fn cmd_run(id: Option<u32>, all: bool, auto_merge: bool) -> anyhow::Result<()> {
    let repo = std::env::current_dir()?;
    let rloop_dir = repo.join(".rloop");
    let config = rloop::config::load_config(&rloop_dir)?;

    if all {
        tokio::select! {
            result = rloop::session::run_all_session(&repo, &config, auto_merge) => {
                let result = result?;
                print_session_result(&result);
            }
            _ = tokio::signal::ctrl_c() => {
                eprintln!("\nCancelled. Cleaning up...");
                cleanup_worktrees(&repo);
            }
        }
        return Ok(());
    }

    let Some(task_id) = id else {
        anyhow::bail!("specify a task ID or use --all");
    };
    let target = rloop::task::TaskId(task_id);

    tokio::select! {
        result = rloop::session::run_session(&repo, target, &config, auto_merge) => {
            let result = result?;
            print_session_result(&result);
        }
        _ = tokio::signal::ctrl_c() => {
            eprintln!("\nCancelled. Cleaning up...");
            cleanup_worktrees(&repo);
        }
    }

    Ok(())
}

fn print_session_result(result: &rloop::session::SessionResult) {
    eprintln!("Session {} complete", result.session_id);
    eprintln!("  Branch: {}", result.branch);
    eprintln!("  Completed: {:?}", result.completed_tasks);
    if !result.failed_tasks.is_empty() {
        eprintln!("  Failed: {:?}", result.failed_tasks);
    }
    if !result.skipped_tasks.is_empty() {
        eprintln!("  Skipped: {:?}", result.skipped_tasks);
    }
    if result.auto_merged {
        eprintln!("  Auto-merged to parent branch");
    }
}

/// Best-effort cleanup of any leftover worktrees in `.rloop/worktrees/`.
fn cleanup_worktrees(repo: &std::path::Path) {
    let worktrees_dir = repo.join(".rloop").join("worktrees");
    let Ok(entries) = std::fs::read_dir(&worktrees_dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Parse task id from directory name
        let Ok(id) = name.parse::<u32>() else {
            continue;
        };
        let wt = rloop::git::Worktree {
            path: path.clone(),
            branch: format!("rloop/wt/{name}"),
            task_id: rloop::task::TaskId(id),
        };
        if let Err(e) = rloop::git::cleanup_worktree(repo, &wt) {
            eprintln!("warning: failed to cleanup worktree {name}: {e}");
        }
    }
}

fn cmd_list() -> anyhow::Result<()> {
    let repo = std::env::current_dir()?;
    let tasks_dir = repo.join(".rloop").join("tasks");

    if !tasks_dir.exists() {
        eprintln!("No .rloop/tasks/ directory found. Run `rloop init` first.");
        return Ok(());
    }

    let tasks = rloop::task::load_all_tasks(&tasks_dir)?;

    if tasks.is_empty() {
        eprintln!("No tasks found.");
        return Ok(());
    }

    for task in &tasks {
        let status = if task.completed { "done" } else { "    " };
        let first_line = task
            .description
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("(no description)");
        eprintln!("[{status}] {:>2} - {first_line}", task.id);
    }

    Ok(())
}
