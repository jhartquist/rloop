use std::fmt;

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
    /// Manage tasks
    Task {
        #[command(subcommand)]
        action: TaskAction,
    },
}

#[derive(Subcommand)]
enum TaskAction {
    /// Create a new task file
    New {
        /// Task description (becomes the markdown body)
        description: String,
        /// Short descriptive title for the task
        #[arg(long)]
        title: Option<String>,
        /// Model to use (Haiku, Sonnet, Opus)
        #[arg(long)]
        model: Option<ModelArg>,
        /// Task IDs this task depends on
        #[arg(long = "depends-on", value_delimiter = ',')]
        depends_on: Vec<u32>,
        /// Verification command(s)
        #[arg(long)]
        verification: Vec<String>,
    },
    /// Show full details of a task
    Show {
        /// Task ID to show (e.g., 3 for task 03)
        id: u32,
    },
    /// List tasks and their status
    List,
}

/// Wrapper for Model CLI parsing (case-insensitive display in help).
#[derive(Clone)]
struct ModelArg(rloop::model::Model);

impl fmt::Display for ModelArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for ModelArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Haiku" | "haiku" => Ok(Self(rloop::model::Model::Haiku)),
            "Sonnet" | "sonnet" => Ok(Self(rloop::model::Model::Sonnet)),
            "Opus" | "opus" => Ok(Self(rloop::model::Model::Opus)),
            _ => Err(format!(
                "unknown model: {s} (expected Haiku, Sonnet, or Opus)"
            )),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Ensure panics are always visible on stderr, even if a TUI or alternate
    // screen is active in the future. This hook runs before the default one
    // would, guaranteeing the message reaches the user.
    std::panic::set_hook(Box::new(|info| {
        eprintln!("\nrloop panicked: {info}");
    }));

    let cli = Cli::parse();

    match cli.command {
        Command::Init => cmd_init(),
        Command::Run {
            id,
            all,
            auto_merge,
        } => cmd_run(id, all, auto_merge).await,
        Command::Task { action } => match action {
            TaskAction::New {
                description,
                title,
                model,
                depends_on,
                verification,
            } => cmd_task_new(
                &description,
                title.as_deref(),
                model,
                &depends_on,
                &verification,
            ),
            TaskAction::Show { id } => cmd_task_show(id),
            TaskAction::List => cmd_list(),
        },
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
                cleanup_lock(&repo);
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
            cleanup_lock(&repo);
        }
    }

    Ok(())
}

fn print_session_result(result: &rloop::session::SessionResult) {
    eprintln!("\nSession {} complete", result.session_id);
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

    let m = &result.metrics;
    let secs = m.duration.as_secs();
    let dur = if secs >= 60 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    };
    eprintln!("  Duration: {dur}  Cost: ${:.2}", m.cost_usd);
    eprintln!(
        "  Tokens: {} in / {} out / {} cache-read / {} cache-create",
        fmt_tokens(m.input_tokens),
        fmt_tokens(m.output_tokens),
        fmt_tokens(m.cache_read_tokens),
        fmt_tokens(m.cache_creation_tokens),
    );
}

fn fmt_tokens(n: u64) -> String {
    if n >= 1000 {
        #[allow(clippy::cast_precision_loss)]
        let k = n as f64 / 1000.0;
        format!("{k:.0}k")
    } else {
        n.to_string()
    }
}

/// Best-effort cleanup of the session lock file.
fn cleanup_lock(repo: &std::path::Path) {
    let lock_path = repo.join(".rloop").join(".lock");
    if lock_path.exists()
        && let Err(e) = std::fs::remove_file(&lock_path)
    {
        eprintln!("warning: failed to remove lock file: {e}");
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
            branch: format!("rloop-wt-{name}"),
            task_id: rloop::task::TaskId(id),
        };
        if let Err(e) = rloop::git::cleanup_worktree(repo, &wt) {
            eprintln!("warning: failed to cleanup worktree {name}: {e}");
        }
    }
}

fn cmd_task_new(
    description: &str,
    title: Option<&str>,
    model: Option<ModelArg>,
    depends_on: &[u32],
    verification: &[String],
) -> anyhow::Result<()> {
    use std::fmt::Write;

    let repo = std::env::current_dir()?;
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir)?;

    let next_id = next_task_id(&tasks_dir)?;
    let filename = format!("{next_id:02}.md");

    let model_str = model.map_or_else(
        || rloop::model::Model::Opus.to_string(),
        |m| m.0.to_string(),
    );

    let mut frontmatter = format!("---\nid: {next_id}\n");

    if let Some(t) = title {
        writeln!(frontmatter, "title: {t}").expect("write to String never fails");
    }

    writeln!(frontmatter, "model: {model_str}").expect("write to String never fails");

    if !depends_on.is_empty() {
        let deps: Vec<String> = depends_on.iter().map(ToString::to_string).collect();
        writeln!(frontmatter, "depends_on: [{}]", deps.join(", "))
            .expect("write to String never fails");
    }

    if !verification.is_empty() {
        if verification.len() == 1 {
            writeln!(frontmatter, "verification: \"{}\"", verification[0])
                .expect("write to String never fails");
        } else {
            let quoted: Vec<String> = verification.iter().map(|v| format!("\"{v}\"")).collect();
            writeln!(frontmatter, "verification: [{}]", quoted.join(", "))
                .expect("write to String never fails");
        }
    }

    frontmatter.push_str("completed: false\n---\n");

    let content = format!("{frontmatter}\n{description}\n");
    let path = tasks_dir.join(&filename);
    std::fs::write(&path, content)?;

    eprintln!("Created task {next_id:02} at {}", path.display());
    Ok(())
}

fn cmd_task_show(id: u32) -> anyhow::Result<()> {
    let repo = std::env::current_dir()?;
    let tasks_dir = repo.join(".rloop").join("tasks");

    if !tasks_dir.exists() {
        anyhow::bail!("No .rloop/tasks/ directory found. Run `rloop init` first.");
    }

    let target = rloop::task::TaskId(id);
    let task_path = tasks_dir.join(format!("{target}.md"));

    let task = rloop::task::load_task(&task_path)
        .map_err(|_| anyhow::anyhow!("task {target} not found"))?;

    // Header: Task ID and title
    if task.title.is_empty() {
        println!("Task {}", task.id);
    } else {
        println!("Task {}: {}", task.id, task.title);
    }
    println!();

    // Frontmatter fields
    let status = if task.completed {
        "completed"
    } else {
        "incomplete"
    };
    println!("Status:     {status}");
    println!("Model:      {}", task.model);

    if task.depends_on.is_empty() {
        println!("Depends on: (none)");
    } else {
        let deps: Vec<String> = task.depends_on.iter().map(ToString::to_string).collect();
        println!("Depends on: {}", deps.join(", "));
    }

    if let Some(ref cmds) = task.verification {
        println!("Verify:     {}", cmds.0.join("; "));
    } else {
        println!("Verify:     (none)");
    }

    // Separator and full description
    println!();
    println!("---");
    println!();
    print!("{}", task.description);

    Ok(())
}

fn next_task_id(tasks_dir: &std::path::Path) -> anyhow::Result<u32> {
    if !tasks_dir.exists() {
        return Ok(0);
    }
    let tasks = rloop::task::load_all_tasks(tasks_dir)?;
    Ok(tasks.last().map_or(0, |t| t.id.0 + 1))
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
        let summary = if task.title.is_empty() {
            task.description
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("(no description)")
        } else {
            &task.title
        };
        eprintln!("[{status}] {:>2} - {summary}", task.id);
    }

    Ok(())
}
