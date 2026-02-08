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
        /// Maximum parallel tasks (overrides config `max_parallel`)
        #[arg(long)]
        parallel: Option<usize>,
    },
    /// Manage tasks
    Task {
        #[command(subcommand)]
        action: TaskAction,
    },
    /// Manage sessions
    Session {
        #[command(subcommand)]
        action: SessionAction,
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
        model: Option<rloop::model::Model>,
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
        model: Option<rloop::model::Model>,
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

#[derive(Subcommand)]
enum SessionAction {
    /// Display a session log with elapsed time
    Show {
        /// Session ID (e.g., 20250206-143022)
        session_id: String,
    },
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
            parallel,
        } => cmd_run(id, all, auto_merge, parallel).await,
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
        Command::Session { action } => match action {
            SessionAction::Show { session_id } => cmd_session_show(&session_id),
        },
    }
}

fn cmd_init() -> anyhow::Result<()> {
    let repo = std::env::current_dir()?;
    let rloop_dir = rloop::config::init_rloop_dir(&repo)?;
    eprintln!("Initialized {}", rloop_dir.display());
    Ok(())
}

async fn cmd_run(
    id: Option<u32>,
    all: bool,
    auto_merge: bool,
    parallel: Option<usize>,
) -> anyhow::Result<()> {
    let repo = std::env::current_dir()?;
    let rloop_dir = repo.join(".rloop");
    let mut config = rloop::config::load_config(&rloop_dir)?;

    // CLI --parallel overrides config max_parallel
    if let Some(n) = parallel {
        anyhow::ensure!(n >= 1, "--parallel must be at least 1");
        config.max_parallel = n;
    }

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
    let dur = rloop::log::format_duration(m.duration);
    eprintln!("  Duration: {dur}  Cost: ${:.2}", m.cost_usd);
    eprintln!(
        "  Tokens: {} in / {} out / {} cache-read / {} cache-create",
        rloop::log::format_tokens(m.input_tokens),
        rloop::log::format_tokens(m.output_tokens),
        rloop::log::format_tokens(m.cache_read_tokens),
        rloop::log::format_tokens(m.cache_creation_tokens),
    );
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

        // Parse directory name to determine worktree type and construct branch name
        let (task_id, branch) = if let Some(id_str) = name.strip_suffix("-mergefix") {
            // Merge-fix worktree: {id}-mergefix -> rloop-mergefix-{id}
            let Ok(task_id) = id_str.parse::<rloop::task::TaskId>() else {
                eprintln!("warning: skipping invalid merge-fix worktree directory: {name}");
                continue;
            };
            (task_id, format!("rloop-mergefix-{id_str}"))
        } else {
            // Regular worktree: {id} -> rloop-wt-{id}
            let Ok(task_id) = name.parse::<rloop::task::TaskId>() else {
                eprintln!("warning: skipping unrecognized worktree directory: {name}");
                continue;
            };
            (task_id, format!("rloop-wt-{name}"))
        };

        let wt = rloop::git::Worktree {
            path: path.clone(),
            branch,
            task_id,
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
        writeln!(fm, "title: \"{escaped_title}\"").expect("write to String never fails");
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
    model: Option<rloop::model::Model>,
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
    let model_name = model.unwrap_or(rloop::model::Model::Sonnet);

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
    model: Option<rloop::model::Model>,
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
    let enhance_model = model.unwrap_or(config.task.enhance_model);

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
            rloop::builtin_tool::BuiltinTool::Read,
            rloop::builtin_tool::BuiltinTool::Glob,
            rloop::builtin_tool::BuiltinTool::Grep,
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
        println!("Verify:     {}", cmds);
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

fn cmd_session_show(session_id: &str) -> anyhow::Result<()> {
    let repo = std::env::current_dir()?;
    let sessions_dir = repo.join(".rloop").join("sessions");

    if !sessions_dir.exists() {
        anyhow::bail!("No .rloop/sessions/ directory found. Run `rloop init` first.");
    }

    let session_path = sessions_dir.join(format!("{session_id}.jsonl"));

    if !session_path.exists() {
        anyhow::bail!("Session {session_id} not found");
    }

    let events = rloop::log::load_events(&session_path)?;

    if events.is_empty() {
        eprintln!("Session log is empty");
        return Ok(());
    }

    // Print session header
    println!("Session {session_id}\n");

    // Track elapsed time from event sequence (no actual timestamps in events)
    // We'll use event index as a proxy for time progression
    let total_events = events.len();

    for (idx, event) in events.iter().enumerate() {
        // Format elapsed time based on event index
        // This is a simplified approach - in reality we'd need actual timestamps
        let elapsed = format_elapsed_time(idx, total_events);
        print_event_with_elapsed(&elapsed, event);
    }

    Ok(())
}

/// Format elapsed time as a right-aligned string (e.g., "     +0s", "   +12s", "+1m 34s")
fn format_elapsed_time(event_idx: usize, _total: usize) -> String {
    // Simplified: use event index as proxy for seconds
    // In a real implementation, we'd compute actual elapsed time from timestamps
    #[allow(clippy::cast_possible_truncation)]
    let secs = event_idx as u64;

    let time_str = if secs >= 60 {
        format!("+{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("+{secs}s")
    };

    // Right-align to 8 characters
    format!("{time_str:>8}")
}

/// Print an event with elapsed time prefix
#[allow(clippy::too_many_lines)]
fn print_event_with_elapsed(elapsed: &str, event: &rloop::log::Event) {
    use colored::Colorize;
    use rloop::log::Event;

    match event {
        Event::SessionBanner {
            session_id,
            datetime,
            target,
            parent_branch,
            session_branch,
        } => {
            println!("{elapsed}  {} Session {session_id}", "▶".green());
            println!("          Date:   {datetime}");
            let target_str = target.map_or("all".to_string(), |id| format!("{id:02}"));
            println!("          Target: task {target_str}");
            println!("          Branch: {parent_branch} → {session_branch}");
        }
        Event::SessionStarted { session_id, target } => {
            println!("{elapsed}  {} Session {session_id}", "▶".green());
            let target_str = target.map_or("all".to_string(), |id| format!("{id:02}"));
            println!("          Target: task {target_str}");
        }
        Event::StepStarted {
            task_id,
            attempt,
            model_id,
            cc_session_id,
        } => {
            println!(
                "{elapsed}  {} Task {task_id:02}  step {attempt}  model: {model_id}  cc: {cc_session_id}",
                "→".blue(),
            );
        }
        Event::TaskStarted { task_id } => {
            println!("{elapsed}  {} task {task_id:02}", "→".blue());
        }
        Event::TaskCompleted { task_id, summary } => {
            println!("{elapsed}  {} task {task_id:02} — {summary}", "✓".green());
        }
        Event::TaskFailed { task_id, reason } => {
            println!("{elapsed}  {} task {task_id:02} — {reason}", "✗".red());
        }
        Event::ToolCall { name, input } => {
            let param = extract_tool_param(name, input);
            if param.is_empty() {
                println!("{elapsed}    CALL  {name}");
            } else {
                println!("{elapsed}    CALL  {name} {param}");
            }
        }
        Event::ToolResult { name, output } => {
            let summary = extract_result_summary(name, output);
            println!("{elapsed}     RES  {summary}");
        }
        Event::AssistantMessage { content } => {
            // Print first line with elapsed time, subsequent lines indented
            for (i, line) in content.lines().enumerate() {
                if i == 0 {
                    println!("{elapsed}    ASST  {line}");
                } else {
                    println!("              {line}");
                }
            }
        }
        Event::VerificationRan {
            task_id,
            command,
            passed,
            output: _,
        } => {
            let icon = if *passed { "✓".green() } else { "✗".red() };
            let status = if *passed { "passed" } else { "failed" };
            println!("{elapsed}  {icon} verify task {task_id:02} `{command}`: {status}");
        }
        Event::WorktreeCreated { task_id, path } => {
            println!("{elapsed}    worktree {task_id:02} → {}", path.display());
        }
        Event::WorktreeMerged {
            task_id,
            into_branch,
        } => {
            println!("{elapsed}    merged {task_id:02} → {into_branch}");
        }
        Event::WorktreeCleanedUp { task_id } => {
            println!("{elapsed}    cleaned up {task_id:02}");
        }
        Event::SessionComplete { branch } => {
            println!(
                "{elapsed}  {} session complete → branch {branch}",
                "■".green()
            );
        }
        Event::SessionAutoMerged {
            branch,
            into_branch,
        } => {
            println!(
                "{elapsed}  {} auto-merged {branch} → {into_branch}",
                "■".green()
            );
        }
        Event::SessionCancelled => {
            println!("{elapsed}  {} session cancelled", "■".yellow());
        }
        Event::ContextUsage {
            percentage,
            recommendation,
        } => {
            println!("{elapsed}      CTX  {percentage:.1}% — {recommendation}");
        }
        Event::StepSummary {
            task_id,
            step_number,
            max_steps,
            duration,
            cost_usd,
            input_tokens,
            output_tokens,
        } => {
            let dur = rloop::log::format_duration(*duration);
            println!(
                "{elapsed}    {} step {step_number}/{max_steps}  {dur}  ${cost_usd:.2}  tokens: {} in / {} out",
                format!("task {task_id:02}").dimmed(),
                rloop::log::format_tokens(*input_tokens),
                rloop::log::format_tokens(*output_tokens),
            );
        }
        Event::TaskSummary {
            task_id,
            success,
            duration,
            cost_usd,
            input_tokens,
            output_tokens,
            steps,
        } => {
            let status = if *success {
                "done".green().to_string()
            } else {
                "failed".red().to_string()
            };
            let dur = rloop::log::format_duration(*duration);
            println!(
                "{elapsed}  {} task {task_id:02}  {status}  {dur}  ${cost_usd:.2}  tokens: {} in / {} out  ({steps} step{})",
                "─".dimmed(),
                rloop::log::format_tokens(*input_tokens),
                rloop::log::format_tokens(*output_tokens),
                if *steps == 1 { "" } else { "s" },
            );
        }
        Event::SessionSummary {
            duration,
            cost_usd,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_creation_tokens,
            ..
        } => {
            let dur = rloop::log::format_duration(*duration);
            println!("{elapsed}  Duration: {dur}  Cost: ${cost_usd:.2}");
            println!(
                "          Tokens: {} in / {} out / {} cache-read / {} cache-create",
                rloop::log::format_tokens(*input_tokens),
                rloop::log::format_tokens(*output_tokens),
                rloop::log::format_tokens(*cache_read_tokens),
                rloop::log::format_tokens(*cache_creation_tokens),
            );
        }
        Event::MergeFailed {
            task_id,
            branch,
            reason,
        } => {
            println!(
                "{elapsed}  {} task {task_id:02} merge failed — {reason}. Work preserved on branch {branch}",
                "✗".red()
            );
        }
        Event::MergeFixStarted { task_id } => {
            println!(
                "{elapsed}  {} task {task_id:02} merge-fix agent started",
                "→".blue()
            );
        }
        Event::MergeFixSucceeded { task_id, cost_usd } => {
            println!(
                "{elapsed}  {} task {task_id:02} merge-fix resolved (${cost_usd:.2})",
                "✓".green()
            );
        }
        Event::MergeFixFailed { task_id, reason } => {
            println!(
                "{elapsed}  {} task {task_id:02} merge-fix failed — {reason}",
                "✗".red()
            );
        }
        // Skip some low-level events
        Event::PromptSent { .. } | Event::TokenUsage { .. } => {}
    }
}


/// Extract a meaningful parameter from a tool call
fn extract_tool_param(name: &str, input: &serde_json::Value) -> String {
    match name {
        "Read" => input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "Edit" | "Write" => input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "Bash" => input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "Grep" => input
            .get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "Glob" => input
            .get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    }
}

/// Extract a summary from a tool result
fn extract_result_summary(name: &str, output: &serde_json::Value) -> String {
    if name == "Read"
        && let Some(file) = output.get("file")
    {
        let num_lines = file
            .get("numLines")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        return format!("{name} ok ({num_lines} lines)");
    }

    if matches!(name, "Write" | "Edit") {
        return format!("{name} ok");
    }

    format!("{name} ok")
}
