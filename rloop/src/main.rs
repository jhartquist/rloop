use std::fmt;

use anyhow::Context;
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
    /// Create new task(s) from a rough description via LLM enhancement
    New {
        /// Rough description of what to build (LLM will refine into task specs)
        description: String,
        /// Model for the enhancement query (overrides config `enhance_model`)
        #[arg(long)]
        model: Option<ModelArg>,
        /// Additional dependency task IDs applied to all created tasks
        #[arg(long = "depends-on", value_delimiter = ',')]
        depends_on: Vec<u32>,
        /// Verification command(s) applied to all created tasks
        #[arg(long)]
        verification: Vec<String>,
    },
    /// Add a task from explicit fields (no LLM, deterministic)
    Add {
        /// Short descriptive title (required)
        #[arg(long)]
        title: String,
        /// Task body/description (reads from stdin if omitted)
        #[arg(long)]
        body: Option<String>,
        /// Model for execution (default: Sonnet)
        #[arg(long)]
        model: Option<ModelArg>,
        /// Dependency task IDs
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
                model,
                depends_on,
                verification,
            } => {
                let repo = std::env::current_dir()?;
                let rloop_dir = repo.join(".rloop");
                let config = rloop::config::load_config(&rloop_dir)?;
                cmd_task_new(
                    &repo,
                    &description,
                    model,
                    &depends_on,
                    &verification,
                    &config,
                )
                .await
            }
            TaskAction::Add {
                title,
                body,
                model,
                depends_on,
                verification,
            } => {
                let repo = std::env::current_dir()?;
                cmd_task_add(
                    &repo,
                    &title,
                    body.as_deref(),
                    model,
                    &depends_on,
                    &verification,
                )
            }
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

/// Structured output from the LLM enhancement query.
#[derive(serde::Deserialize)]
struct EnhancedTasks {
    tasks: Vec<EnhancedTask>,
}

#[derive(serde::Deserialize)]
struct EnhancedTask {
    title: String,
    description: String,
    depends_on: Vec<u32>,
    model: String,
}

/// Build the structured output schema for `EnhancedTasks`.
fn enhanced_tasks_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "json_schema",
        "schema": {
            "type": "object",
            "properties": {
                "tasks": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": { "type": "string", "description": "Short descriptive title" },
                            "description": { "type": "string", "description": "Full markdown task description with acceptance criteria" },
                            "depends_on": {
                                "type": "array",
                                "items": { "type": "integer" },
                                "description": "Task IDs this task depends on"
                            },
                            "model": {
                                "type": "string",
                                "enum": ["Haiku", "Sonnet", "Opus"],
                                "description": "Model to use for execution"
                            }
                        },
                        "required": ["title", "description", "depends_on", "model"]
                    }
                }
            },
            "required": ["tasks"]
        }
    })
}

/// Build the prompt for the LLM enhancement query.
fn build_enhance_prompt(
    description: &str,
    existing_tasks: &[rloop::task::Task],
    extra_depends_on: &[u32],
    verification: &[String],
    next_id: u32,
) -> String {
    use std::fmt::Write;

    let mut prompt = String::from(
        "You are a task planning assistant for rloop, a DAG-based task executor. \
         Your job is to take a rough description and produce one or more well-structured task specs.\n\n\
         Use the Read, Glob, and Grep tools to explore the codebase and understand the project \
         before generating tasks. This helps you write precise, actionable task descriptions.\n\n",
    );

    if !existing_tasks.is_empty() {
        prompt.push_str("## Existing tasks\n\n");
        for task in existing_tasks {
            let status = if task.completed { "done" } else { "todo" };
            let deps: Vec<String> = task.depends_on.iter().map(ToString::to_string).collect();
            let dep_str = if deps.is_empty() {
                String::new()
            } else {
                format!(" depends_on=[{}]", deps.join(","))
            };
            writeln!(
                prompt,
                "- {id}: {title} [{status}]{dep_str}",
                id = task.id,
                title = if task.title.is_empty() {
                    "(untitled)"
                } else {
                    &task.title
                },
            )
            .expect("write to String never fails");
        }
        prompt.push('\n');
    }

    writeln!(
        prompt,
        "## Request\n\n{description}\n\n\
         ## Instructions\n\n\
         Generate one or more tasks from the request above. For each task provide:\n\
         - **title**: short descriptive title\n\
         - **description**: full markdown body with context and acceptance criteria\n\
         - **depends_on**: task IDs this depends on (from existing tasks or other new tasks you create)\n\
         - **model**: which model to use (Haiku for simple, Sonnet for moderate, Opus for complex)\n\n\
         New tasks will be assigned sequential IDs starting at {next_id}. \
         If you create multiple tasks that depend on each other, reference the new IDs.\n",
    )
    .expect("write to String never fails");

    if !extra_depends_on.is_empty() {
        let deps: Vec<String> = extra_depends_on.iter().map(ToString::to_string).collect();
        writeln!(
            prompt,
            "All tasks must include these additional dependencies: [{}]\n",
            deps.join(", ")
        )
        .expect("write to String never fails");
    }

    if !verification.is_empty() {
        let cmds = verification.join("; ");
        writeln!(prompt, "All tasks will use this verification: \"{cmds}\"\n",)
            .expect("write to String never fails");
    }

    prompt
}

/// Build a task file content string from an `EnhancedTask`.
fn build_task_file(
    id: u32,
    task: &EnhancedTask,
    extra_depends_on: &[u32],
    verification: &[String],
) -> String {
    use std::fmt::Write;

    let mut fm = format!("---\nid: {id}\n");

    if !task.title.is_empty() {
        let escaped_title = task.title.replace('\\', "\\\\").replace('"', "\\\"");
        writeln!(fm, "title: \"{}\"", escaped_title).expect("write to String never fails");
    }

    writeln!(fm, "model: {}", task.model).expect("write to String never fails");

    // Merge task deps with extra CLI deps, dedup
    let mut all_deps: Vec<u32> = task.depends_on.clone();
    for &dep in extra_depends_on {
        if !all_deps.contains(&dep) {
            all_deps.push(dep);
        }
    }
    if !all_deps.is_empty() {
        let deps: Vec<String> = all_deps.iter().map(ToString::to_string).collect();
        writeln!(fm, "depends_on: [{}]", deps.join(", ")).expect("write to String never fails");
    }

    if !verification.is_empty() {
        if verification.len() == 1 {
            writeln!(fm, "verification: \"{}\"", verification[0])
                .expect("write to String never fails");
        } else {
            let quoted: Vec<String> = verification.iter().map(|v| format!("\"{v}\"")).collect();
            writeln!(fm, "verification: [{}]", quoted.join(", "))
                .expect("write to String never fails");
        }
    }

    fm.push_str("completed: false\n---\n");

    format!("{fm}\n{}\n", task.description)
}

fn cmd_task_add(
    repo: &std::path::Path,
    title: &str,
    body: Option<&str>,
    model: Option<ModelArg>,
    depends_on: &[u32],
    verification: &[String],
) -> anyhow::Result<()> {
    use std::fmt::Write;

    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir)?;

    let existing_tasks = if tasks_dir.exists() {
        rloop::task::load_all_tasks(&tasks_dir)?
    } else {
        vec![]
    };

    let id = existing_tasks.last().map_or(0, |t| t.id.0 + 1);

    // Read body from stdin if not provided via --body
    let description = if let Some(b) = body {
        b.to_string()
    } else {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("failed to read task body from stdin")?;
        buf
    };

    // Build frontmatter
    let model_name = model.map_or(rloop::model::Model::Sonnet, |m| m.0);

    let escaped_title = title.replace('\\', "\\\\").replace('"', "\\\"");
    let mut fm = format!("---\nid: {id}\ntitle: \"{escaped_title}\"\nmodel: {model_name}\n");

    if !depends_on.is_empty() {
        let deps: Vec<String> = depends_on.iter().map(ToString::to_string).collect();
        writeln!(fm, "depends_on: [{}]", deps.join(", ")).expect("write to String never fails");
    }

    if !verification.is_empty() {
        if verification.len() == 1 {
            writeln!(fm, "verification: \"{}\"", verification[0])
                .expect("write to String never fails");
        } else {
            let quoted: Vec<String> = verification.iter().map(|v| format!("\"{v}\"")).collect();
            writeln!(fm, "verification: [{}]", quoted.join(", "))
                .expect("write to String never fails");
        }
    }

    fm.push_str("completed: false\n---\n");

    let content = format!("{fm}\n{description}");

    // Write file
    let filename = format!("{id:02}.md");
    let path = tasks_dir.join(&filename);
    std::fs::write(&path, &content)?;

    // Git add + commit
    let path_str = path.to_str().context("non-UTF-8 task path")?;
    rloop::git::git(repo, &["add", path_str])?;

    let commit_msg = format!("task: add {id:02} - {title}");
    rloop::git::git(repo, &["commit", "-m", &commit_msg])?;

    eprintln!("Created task {id:02}: {title}");

    Ok(())
}

async fn cmd_task_new(
    repo: &std::path::Path,
    description: &str,
    model: Option<ModelArg>,
    depends_on: &[u32],
    verification: &[String],
    config: &rloop::config::Config,
) -> anyhow::Result<()> {
    let tasks_dir = repo.join(".rloop").join("tasks");
    std::fs::create_dir_all(&tasks_dir)?;

    // Load existing tasks for context
    let existing_tasks = if tasks_dir.exists() {
        rloop::task::load_all_tasks(&tasks_dir)?
    } else {
        vec![]
    };

    let next_id = existing_tasks.last().map_or(0, |t| t.id.0 + 1);

    // Build and run the LLM enhancement query
    let enhance_model = model.map_or(config.task.enhance_model, |m| m.0);

    let prompt = build_enhance_prompt(
        description,
        &existing_tasks,
        depends_on,
        verification,
        next_id,
    );

    eprintln!("Enhancing task description with {enhance_model}...");

    let query_config = rloop::query::QueryConfig {
        model: enhance_model,
        cwd: repo.to_path_buf(),
        tools: vec![
            rloop::tool::BuiltinTool::Read,
            rloop::tool::BuiltinTool::Glob,
            rloop::tool::BuiltinTool::Grep,
        ],
        output_format: Some(enhanced_tasks_schema()),
    };

    let result = rloop::query::run_query(&prompt, &query_config).await?;

    let output = result
        .structured_output
        .context("LLM query did not return structured output")?;
    let enhanced: EnhancedTasks =
        serde_json::from_value(output).context("failed to parse enhanced tasks from LLM output")?;

    if enhanced.tasks.is_empty() {
        anyhow::bail!("LLM returned no tasks");
    }

    // Write task files
    let mut created_paths: Vec<std::path::PathBuf> = Vec::new();
    let mut created_ids: Vec<(u32, String)> = Vec::new();

    for (i, task) in enhanced.tasks.iter().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        let id = next_id + i as u32;
        let filename = format!("{id:02}.md");
        let content = build_task_file(id, task, depends_on, verification);
        let path = tasks_dir.join(&filename);
        std::fs::write(&path, &content)?;
        created_paths.push(path);
        created_ids.push((id, task.title.clone()));
    }

    // Git add all new files and commit
    let path_strs: Vec<&str> = created_paths
        .iter()
        .map(|p| p.to_str().context("non-UTF-8 task path"))
        .collect::<anyhow::Result<Vec<_>>>()?;

    let mut add_args = vec!["add"];
    add_args.extend_from_slice(&path_strs);
    rloop::git::git(repo, &add_args)?;

    let commit_msg = if created_ids.len() == 1 {
        let (id, ref title) = created_ids[0];
        format!("task: add {id:02} - {title}")
    } else {
        let ids: Vec<String> = created_ids
            .iter()
            .map(|(id, _)| format!("{id:02}"))
            .collect();
        format!("task: add {}", ids.join(", "))
    };
    rloop::git::git(repo, &["commit", "-m", &commit_msg])?;

    // Print summary
    eprintln!("\nCreated {} task(s):", created_ids.len());
    for (id, title) in &created_ids {
        eprintln!("  {id:02} - {title}");
    }

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
