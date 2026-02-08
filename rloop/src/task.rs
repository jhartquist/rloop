use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde::{Deserialize, Deserializer, Serialize};

use crate::model::Model;

/// Unique identifier for a task. Sequential number matching the task filename.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub u32);

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}", self.0)
    }
}

/// Ordered list of shell commands to verify a task's output.
#[derive(Clone, Debug, Deserialize)]
pub struct VerificationCommands(pub Vec<String>);

/// A task loaded from a markdown file with YAML frontmatter.
#[derive(Clone, Debug)]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    pub model: Model,
    pub depends_on: Vec<TaskId>,
    pub verification: Option<VerificationCommands>,
    pub completed: bool,
    pub description: String,
    /// Path to the `.md` file this task was loaded from.
    /// Used to write `completed: true` back on the session branch.
    pub source_path: PathBuf,
}

/// Ordered execution plan from DAG resolution.
pub struct ExecutionPlan {
    /// The target task, or None if this is an --all session
    pub target: Option<TaskId>,
    pub steps: Vec<TaskId>,
}

// ---------------------------------------------------------------------------
// Private: frontmatter parsing
// ---------------------------------------------------------------------------

fn default_model() -> Model {
    Model::Sonnet
}

/// Deserialize verification from either a single string or a list of strings.
fn deserialize_verification<'de, D>(
    deserializer: D,
) -> Result<Option<VerificationCommands>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrVec {
        Single(String),
        Multiple(Vec<String>),
    }

    let maybe: Option<StringOrVec> = Option::deserialize(deserializer)?;
    Ok(maybe.map(|v| match v {
        StringOrVec::Single(s) => VerificationCommands(vec![s]),
        StringOrVec::Multiple(list) => VerificationCommands(list),
    }))
}

#[derive(Deserialize)]
struct TaskFrontmatter {
    id: u32,
    #[serde(default)]
    title: String,
    #[serde(default = "default_model")]
    model: Model,
    #[serde(default)]
    depends_on: Vec<u32>,
    #[serde(default, deserialize_with = "deserialize_verification")]
    verification: Option<VerificationCommands>,
    #[serde(default)]
    completed: bool,
}

/// Split file content into YAML block and description body.
/// Returns `Err` if no valid `---` delimited frontmatter is found.
fn split_frontmatter(content: &str) -> anyhow::Result<(&str, &str)> {
    let trimmed = content.trim_start();
    let rest = trimmed
        .strip_prefix("---")
        .context("no opening --- delimiter")?;

    // Find the closing ---
    let closing = rest.find("\n---").context("no closing --- delimiter")?;

    let yaml = &rest[..closing];
    let after = &rest[closing + 4..]; // skip past "\n---"

    // Strip one leading newline from the description if present
    let description = after.strip_prefix('\n').unwrap_or(after);

    Ok((yaml, description))
}

/// Load a single task from a markdown file.
///
/// # Errors
/// Returns an error if the file cannot be read, has invalid frontmatter, or contains invalid YAML.
pub fn load_task(path: impl AsRef<Path>) -> anyhow::Result<Task> {
    let path = path.as_ref();
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read task file: {}", path.display()))?;

    if content.is_empty() {
        bail!("task file is empty: {}", path.display());
    }

    let (yaml, description) = split_frontmatter(&content)
        .with_context(|| format!("invalid frontmatter in {}", path.display()))?;

    let fm: TaskFrontmatter = serde_yaml::from_str(yaml)
        .with_context(|| format!("failed to parse YAML in {}", path.display()))?;

    Ok(Task {
        id: TaskId(fm.id),
        title: fm.title,
        model: fm.model,
        depends_on: fm.depends_on.into_iter().map(TaskId).collect(),
        verification: fm.verification,
        completed: fm.completed,
        description: description.to_string(),
        source_path: path.to_path_buf(),
    })
}

/// Returns true if the filename stem is all ASCII digits (e.g. "00", "01", "123").
fn is_task_filename(path: &Path) -> bool {
    let Some(ext) = path.extension() else {
        return false;
    };
    if ext != "md" {
        return false;
    }
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return false;
    };
    !stem.is_empty() && stem.chars().all(|c| c.is_ascii_digit())
}

/// Load all tasks from a directory of markdown files.
///
/// # Errors
/// Returns an error if the directory cannot be read or any task file fails to parse.
pub fn load_all_tasks(dir: impl AsRef<Path>) -> anyhow::Result<Vec<Task>> {
    let dir = dir.as_ref();
    let mut tasks: Vec<Task> = std::fs::read_dir(dir)
        .with_context(|| format!("failed to read task directory: {}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| is_task_filename(p))
        .map(|p| load_task(&p))
        .collect::<anyhow::Result<Vec<_>>>()?;

    tasks.sort_by_key(|t| t.id.0);
    Ok(tasks)
}

/// Update a task file on disk, setting `completed: false` to `completed: true`.
///
/// # Errors
/// Returns an error if the file cannot be read/written or doesn't contain `completed: false`.
pub fn mark_completed(path: impl AsRef<Path>) -> anyhow::Result<()> {
    let path = path.as_ref();
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read task file: {}", path.display()))?;

    if content.contains("completed: true") {
        return Ok(());
    }

    let updated = content.replace("completed: false", "completed: true");

    // If nothing changed and there was no `completed: true`, the field is missing
    if updated == content {
        bail!(
            "task file {} has no `completed` field in frontmatter",
            path.display()
        );
    }

    std::fs::write(path, &updated)
        .with_context(|| format!("failed to write task file: {}", path.display()))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// DAG resolution via DFS topological sort
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Color {
    White,
    Gray,
    Black,
}

/// Resolve the dependency DAG for a target task, returning execution order.
///
/// # Errors
/// Returns an error if the target task is not found, a dependency is missing, or a cycle is detected.
pub fn resolve_dag(tasks: &[Task], target: TaskId) -> anyhow::Result<ExecutionPlan> {
    let task_map: HashMap<TaskId, &Task> = tasks.iter().map(|t| (t.id, t)).collect();

    if !task_map.contains_key(&target) {
        bail!("target task {target} not found");
    }

    let mut colors: HashMap<TaskId, Color> =
        task_map.keys().map(|&id| (id, Color::White)).collect();
    let mut order: Vec<TaskId> = Vec::new();

    dfs(target, &task_map, &mut colors, &mut order)?;

    Ok(ExecutionPlan {
        target: Some(target),
        steps: order,
    })
}

/// Resolve ALL incomplete tasks into a topological execution order.
///
/// This is the `--all` variant: instead of targeting a single task, it finds
/// all incomplete tasks and returns them in dependency order.
///
/// # Errors
/// Returns an error if a dependency is missing or a cycle is detected.
pub fn resolve_all_dag(tasks: &[Task]) -> anyhow::Result<ExecutionPlan> {
    let task_map: HashMap<TaskId, &Task> = tasks.iter().map(|t| (t.id, t)).collect();
    let mut colors: HashMap<TaskId, Color> =
        task_map.keys().map(|&id| (id, Color::White)).collect();
    let mut order: Vec<TaskId> = Vec::new();

    // Visit every incomplete task to get a full topological ordering
    for task in tasks {
        if !task.completed {
            dfs(task.id, &task_map, &mut colors, &mut order)?;
        }
    }

    // Filter out already-completed tasks from the execution plan
    let steps: Vec<TaskId> = order
        .into_iter()
        .filter(|id| !task_map[id].completed)
        .collect();

    Ok(ExecutionPlan {
        target: None,
        steps,
    })
}

fn dfs(
    node: TaskId,
    task_map: &HashMap<TaskId, &Task>,
    colors: &mut HashMap<TaskId, Color>,
    order: &mut Vec<TaskId>,
) -> anyhow::Result<()> {
    let color = colors.get(&node).copied();

    match color {
        Some(Color::Black) => return Ok(()),
        Some(Color::Gray) => bail!("cycle detected: task {node} is part of a dependency cycle"),
        None => bail!("dependency {node} not found in task list"),
        Some(Color::White) => {}
    }

    colors.insert(node, Color::Gray);

    let task = task_map[&node];
    for dep in &task.depends_on {
        dfs(*dep, task_map, colors, order)?;
    }

    colors.insert(node, Color::Black);
    order.push(node);

    Ok(())
}

/// Find tasks in the execution plan that are ready to run.
///
/// A task is ready when:
/// - It appears in `steps` (the execution plan)
/// - It is not in `completed`, `failed`, or `in_flight`
/// - All of its dependencies are in `completed`
///
/// Returns task IDs in the same relative order as `steps`.
#[must_use]
#[allow(clippy::implicit_hasher)]
pub fn find_ready_tasks(
    tasks: &[Task],
    steps: &[TaskId],
    completed: &HashSet<TaskId>,
    failed: &HashSet<TaskId>,
    in_flight: &HashSet<TaskId>,
) -> Vec<TaskId> {
    let task_map: HashMap<TaskId, &Task> = tasks.iter().map(|t| (t.id, t)).collect();

    steps
        .iter()
        .filter(|id| !completed.contains(id))
        .filter(|id| !failed.contains(id))
        .filter(|id| !in_flight.contains(id))
        .filter(|id| {
            task_map
                .get(id)
                .is_some_and(|task| task.depends_on.iter().all(|dep| completed.contains(dep)))
        })
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;

    // --- TaskId tests (implemented, will pass) ---

    #[test]
    fn task_id_display_single_digit() {
        assert_eq!(TaskId(3).to_string(), "03");
    }

    #[test]
    fn task_id_display_double_digit() {
        assert_eq!(TaskId(42).to_string(), "42");
    }

    #[test]
    fn task_id_equality() {
        assert_eq!(TaskId(1), TaskId(1));
        assert_ne!(TaskId(1), TaskId(2));
    }

    #[test]
    fn task_id_serialize_json() {
        let json = serde_json::to_string(&TaskId(3)).unwrap();
        assert_eq!(json, "3");
    }

    // --- load_task tests (will fail — todo) ---

    fn write_task_file(dir: &Path, filename: &str, content: &str) -> PathBuf {
        let path = dir.join(filename);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn parse_minimal_task() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_task_file(dir.path(), "00.md", "---\nid: 0\n---\nDo something.\n");
        let task = load_task(&path).unwrap();
        assert_eq!(task.id, TaskId(0));
        assert_eq!(task.model, Model::Sonnet); // default
        assert!(task.depends_on.is_empty());
        assert!(task.verification.is_none());
        assert!(!task.completed);
        assert_eq!(task.description.trim(), "Do something.");
    }

    #[test]
    fn parse_full_task() {
        let dir = tempfile::tempdir().unwrap();
        let content = r#"---
id: 3
model: Opus
depends_on: [1, 2]
verification: ["cargo test", "cargo clippy"]
completed: true
---

# Implement feature

Do the thing.
"#;
        let path = write_task_file(dir.path(), "03.md", content);
        let task = load_task(&path).unwrap();
        assert_eq!(task.id, TaskId(3));
        assert_eq!(task.model, Model::Opus);
        assert_eq!(task.depends_on, vec![TaskId(1), TaskId(2)]);
        assert_eq!(task.verification.as_ref().unwrap().0.len(), 2);
        assert!(task.completed);
        assert!(task.description.contains("# Implement feature"));
    }

    #[test]
    fn parse_task_preserves_markdown() {
        let dir = tempfile::tempdir().unwrap();
        let body = "# Title\n\n```rust\nfn main() {}\n```\n\n- item 1\n- item 2\n";
        let content = format!("---\nid: 0\n---\n{body}");
        let path = write_task_file(dir.path(), "00.md", &content);
        let task = load_task(&path).unwrap();
        assert!(task.description.contains("```rust"));
        assert!(task.description.contains("- item 1"));
    }

    #[test]
    fn parse_task_missing_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_task_file(dir.path(), "00.md", "---\nmodel: Sonnet\n---\nBody\n");
        assert!(load_task(&path).is_err());
    }

    #[test]
    fn parse_task_invalid_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_task_file(dir.path(), "00.md", "---\n: :\n---\nBody\n");
        assert!(load_task(&path).is_err());
    }

    #[test]
    fn parse_task_no_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_task_file(dir.path(), "00.md", "Just some text without frontmatter\n");
        assert!(load_task(&path).is_err());
    }

    #[test]
    fn parse_task_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_task_file(dir.path(), "00.md", "");
        assert!(load_task(&path).is_err());
    }

    #[test]
    fn parse_task_frontmatter_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_task_file(dir.path(), "00.md", "---\nid: 0\n---\n");
        let task = load_task(&path).unwrap();
        assert_eq!(task.id, TaskId(0));
        assert!(task.description.is_empty() || task.description.trim().is_empty());
    }

    #[test]
    fn parse_task_verification_single() {
        let dir = tempfile::tempdir().unwrap();
        let content = "---\nid: 0\nverification: \"cargo test\"\n---\nBody\n";
        let path = write_task_file(dir.path(), "00.md", content);
        let task = load_task(&path).unwrap();
        let cmds = task.verification.unwrap();
        assert_eq!(cmds.0.len(), 1);
        assert_eq!(cmds.0[0], "cargo test");
    }

    #[test]
    fn parse_task_verification_list() {
        let dir = tempfile::tempdir().unwrap();
        let content = "---\nid: 0\nverification: [\"cargo test\", \"cargo clippy\"]\n---\nBody\n";
        let path = write_task_file(dir.path(), "00.md", content);
        let task = load_task(&path).unwrap();
        let cmds = task.verification.unwrap();
        assert_eq!(cmds.0.len(), 2);
    }

    #[test]
    fn parse_task_source_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_task_file(dir.path(), "05.md", "---\nid: 5\n---\nBody\n");
        let task = load_task(&path).unwrap();
        assert_eq!(task.source_path, path);
    }

    // --- load_all_tasks tests (will fail — todo) ---

    #[test]
    fn load_all_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = load_all_tasks(dir.path()).unwrap();
        assert!(tasks.is_empty());
    }

    #[test]
    fn load_all_multiple() {
        let dir = tempfile::tempdir().unwrap();
        write_task_file(dir.path(), "00.md", "---\nid: 0\n---\nFirst\n");
        write_task_file(dir.path(), "01.md", "---\nid: 1\n---\nSecond\n");
        write_task_file(dir.path(), "02.md", "---\nid: 2\n---\nThird\n");
        let tasks = load_all_tasks(dir.path()).unwrap();
        assert_eq!(tasks.len(), 3);
    }

    #[test]
    fn load_all_ignores_non_task_files() {
        let dir = tempfile::tempdir().unwrap();
        write_task_file(dir.path(), "00.md", "---\nid: 0\n---\nTask\n");
        write_task_file(dir.path(), "README.md", "# Not a task\n");
        write_task_file(dir.path(), "notes.txt", "some notes\n");
        let tasks = load_all_tasks(dir.path()).unwrap();
        assert_eq!(tasks.len(), 1);
    }

    // --- resolve_dag tests (will fail — todo) ---

    fn make_task(id: u32, deps: &[u32]) -> Task {
        Task {
            id: TaskId(id),
            title: String::new(),
            model: Model::Sonnet,
            depends_on: deps.iter().map(|&d| TaskId(d)).collect(),
            verification: None,
            completed: false,
            description: String::new(),
            source_path: PathBuf::from(format!("{id:02}.md")),
        }
    }

    #[test]
    fn dag_single_no_deps() {
        let tasks = vec![make_task(0, &[])];
        let plan = resolve_dag(&tasks, TaskId(0)).unwrap();
        assert_eq!(plan.target, Some(TaskId(0)));
        assert_eq!(plan.steps, vec![TaskId(0)]);
    }

    #[test]
    fn dag_linear_chain() {
        let tasks = vec![
            make_task(0, &[]),
            make_task(1, &[0]),
            make_task(2, &[1]),
            make_task(3, &[2]),
        ];
        let plan = resolve_dag(&tasks, TaskId(3)).unwrap();
        assert_eq!(plan.steps, vec![TaskId(0), TaskId(1), TaskId(2), TaskId(3)]);
    }

    #[test]
    fn dag_diamond() {
        let tasks = vec![
            make_task(0, &[]),
            make_task(1, &[0]),
            make_task(2, &[0]),
            make_task(3, &[1, 2]),
        ];
        let plan = resolve_dag(&tasks, TaskId(3)).unwrap();
        // 0 must come first, 3 must come last, 1 and 2 in between
        assert_eq!(plan.steps[0], TaskId(0));
        assert_eq!(*plan.steps.last().unwrap(), TaskId(3));
        assert!(plan.steps.contains(&TaskId(1)));
        assert!(plan.steps.contains(&TaskId(2)));
    }

    #[test]
    fn dag_cycle_two() {
        let tasks = vec![make_task(0, &[1]), make_task(1, &[0])];
        assert!(resolve_dag(&tasks, TaskId(0)).is_err());
    }

    #[test]
    fn dag_cycle_chain() {
        let tasks = vec![make_task(0, &[2]), make_task(1, &[0]), make_task(2, &[1])];
        assert!(resolve_dag(&tasks, TaskId(0)).is_err());
    }

    #[test]
    fn dag_missing_dep() {
        let tasks = vec![make_task(1, &[0])];
        assert!(resolve_dag(&tasks, TaskId(1)).is_err());
    }

    #[test]
    fn dag_self_dep() {
        let tasks = vec![make_task(0, &[0])];
        assert!(resolve_dag(&tasks, TaskId(0)).is_err());
    }

    #[test]
    fn dag_independent_subtree() {
        let tasks = vec![
            make_task(0, &[]),
            make_task(1, &[0]),
            make_task(2, &[]),
            make_task(3, &[2]),
        ];
        let plan = resolve_dag(&tasks, TaskId(1)).unwrap();
        assert_eq!(plan.steps, vec![TaskId(0), TaskId(1)]);
    }

    #[test]
    fn dag_all_simple() {
        let tasks = vec![make_task(0, &[]), make_task(1, &[0]), make_task(2, &[1])];
        let plan = resolve_all_dag(&tasks).unwrap();
        assert_eq!(plan.steps, vec![TaskId(0), TaskId(1), TaskId(2)]);
    }

    #[test]
    fn dag_all_skips_completed() {
        let tasks = vec![
            Task {
                completed: true,
                ..make_task(0, &[])
            },
            make_task(1, &[0]),
            make_task(2, &[1]),
        ];
        let plan = resolve_all_dag(&tasks).unwrap();
        assert_eq!(plan.steps, vec![TaskId(1), TaskId(2)]);
    }

    #[test]
    fn dag_all_empty_when_all_done() {
        let tasks = vec![
            Task {
                completed: true,
                ..make_task(0, &[])
            },
            Task {
                completed: true,
                ..make_task(1, &[0])
            },
        ];
        let plan = resolve_all_dag(&tasks).unwrap();
        assert!(plan.steps.is_empty());
    }

    // --- Edge case: large task count ---

    #[test]
    fn dag_large_linear_chain_100_tasks() {
        let tasks: Vec<Task> = (0..100)
            .map(|i| {
                if i == 0 {
                    make_task(i, &[])
                } else {
                    make_task(i, &[i - 1])
                }
            })
            .collect();

        let plan = resolve_dag(&tasks, TaskId(99)).unwrap();
        assert_eq!(plan.steps.len(), 100);
        assert_eq!(plan.steps[0], TaskId(0));
        assert_eq!(*plan.steps.last().unwrap(), TaskId(99));

        // Verify ordering: each task appears after its dependency
        for (pos, id) in plan.steps.iter().enumerate() {
            if id.0 > 0 {
                let dep_pos = plan
                    .steps
                    .iter()
                    .position(|t| *t == TaskId(id.0 - 1))
                    .unwrap();
                assert!(dep_pos < pos, "task {id} should come after its dependency");
            }
        }
    }

    #[test]
    fn dag_all_large_task_count_150() {
        let tasks: Vec<Task> = (0..150)
            .map(|i| {
                if i == 0 {
                    make_task(i, &[])
                } else {
                    make_task(i, &[i - 1])
                }
            })
            .collect();

        let plan = resolve_all_dag(&tasks).unwrap();
        assert_eq!(plan.steps.len(), 150);
    }

    // --- Edge case: unicode and special characters ---

    #[test]
    fn parse_task_unicode_description() {
        let dir = tempfile::tempdir().unwrap();
        let content = "---\nid: 0\n---\n# 日本語タスク\n\n这是中文描述。\n\nEmoji: 🦀🔧✅\n\nRTL: مرحبا بالعالم\n";
        let path = write_task_file(dir.path(), "00.md", content);
        let task = load_task(&path).unwrap();
        assert!(task.description.contains("日本語タスク"));
        assert!(task.description.contains("这是中文描述"));
        assert!(task.description.contains('🦀'));
        assert!(task.description.contains("مرحبا"));
    }

    #[test]
    fn parse_task_special_chars_in_description() {
        let dir = tempfile::tempdir().unwrap();
        let content = "---\nid: 0\n---\nTabs:\there\nBackslash: C:\\path\\file\nQuotes: \"hello\" 'world'\nAngle: <div>&amp;</div>\n";
        let path = write_task_file(dir.path(), "00.md", content);
        let task = load_task(&path).unwrap();
        assert!(task.description.contains("Tabs:\there"));
        assert!(task.description.contains("C:\\path\\file"));
        assert!(task.description.contains("\"hello\""));
    }

    #[test]
    fn parse_task_empty_description_body() {
        let dir = tempfile::tempdir().unwrap();
        // Frontmatter with only whitespace after it
        let content = "---\nid: 0\n---\n   \n  \n";
        let path = write_task_file(dir.path(), "00.md", content);
        let task = load_task(&path).unwrap();
        assert!(task.description.trim().is_empty());
    }

    // --- Title field tests ---

    #[test]
    fn parse_task_with_title() {
        let dir = tempfile::tempdir().unwrap();
        let content = "---\nid: 0\ntitle: Implement audio preprocessing\n---\nFull description.\n";
        let path = write_task_file(dir.path(), "00.md", content);
        let task = load_task(&path).unwrap();
        assert_eq!(task.title, "Implement audio preprocessing");
    }

    #[test]
    fn parse_task_without_title() {
        let dir = tempfile::tempdir().unwrap();
        let content = "---\nid: 0\n---\nSome description.\n";
        let path = write_task_file(dir.path(), "00.md", content);
        let task = load_task(&path).unwrap();
        assert_eq!(task.title, "");
    }

    #[test]
    fn parse_task_title_with_special_chars() {
        let dir = tempfile::tempdir().unwrap();
        let content = "---\nid: 0\ntitle: \"Add `foo` & bar's \\\"thing\\\"\"\n---\nBody.\n";
        let path = write_task_file(dir.path(), "00.md", content);
        let task = load_task(&path).unwrap();
        assert!(!task.title.is_empty());
    }

    // --- Edge case: TaskId boundaries ---

    #[test]
    fn task_id_display_zero() {
        assert_eq!(TaskId(0).to_string(), "00");
    }

    #[test]
    fn task_id_display_large() {
        // Three-digit task IDs display without padding
        assert_eq!(TaskId(999).to_string(), "999");
    }

    #[test]
    fn task_id_roundtrip_json_boundary() {
        for id in [0, 1, 99, 100, u32::MAX] {
            let tid = TaskId(id);
            let json = serde_json::to_string(&tid).unwrap();
            let back: TaskId = serde_json::from_str(&json).unwrap();
            assert_eq!(back, tid);
        }
    }

    // --- Title with special characters (YAML quoting) ---

    #[test]
    fn parse_task_title_with_colon() {
        let dir = tempfile::tempdir().unwrap();
        // Title with colon must be quoted in YAML
        let content = "---\nid: 5\ntitle: \"Fix: syntax error in parser\"\n---\nBody.\n";
        let path = write_task_file(dir.path(), "05.md", content);
        let task = load_task(&path).unwrap();
        assert_eq!(task.title, "Fix: syntax error in parser");
    }

    #[test]
    fn parse_task_title_with_quotes() {
        let dir = tempfile::tempdir().unwrap();
        // Inner quotes must be escaped with backslash
        let content = "---\nid: 6\ntitle: \"Add \\\"real\\\" validation support\"\n---\nBody.\n";
        let path = write_task_file(dir.path(), "06.md", content);
        let task = load_task(&path).unwrap();
        assert_eq!(task.title, "Add \"real\" validation support");
    }

    #[test]
    fn parse_task_title_with_brackets_and_special() {
        let dir = tempfile::tempdir().unwrap();
        // Title with multiple special characters
        let content =
            "---\nid: 7\ntitle: \"Refactor: [URGENT] fix memory leak - issue #42\"\n---\nBody.\n";
        let path = write_task_file(dir.path(), "07.md", content);
        let task = load_task(&path).unwrap();
        assert_eq!(task.title, "Refactor: [URGENT] fix memory leak - issue #42");
    }

    #[test]
    fn parse_task_title_with_backslash() {
        let dir = tempfile::tempdir().unwrap();
        // Backslash must be escaped
        let content = "---\nid: 8\ntitle: \"Handle \\\\ path separators\"\n---\nBody.\n";
        let path = write_task_file(dir.path(), "08.md", content);
        let task = load_task(&path).unwrap();
        assert_eq!(task.title, "Handle \\ path separators");
    }

    // --- mark_completed tests ---

    #[test]
    fn mark_completed_flips_field() {
        let dir = tempfile::tempdir().unwrap();
        let content = "---\nid: 3\ncompleted: false\n---\nDo stuff.\n";
        let path = write_task_file(dir.path(), "03.md", content);

        mark_completed(&path).unwrap();

        let task = load_task(&path).unwrap();
        assert!(task.completed);
        // Description preserved
        assert_eq!(task.description.trim(), "Do stuff.");
    }

    #[test]
    fn mark_completed_already_true() {
        let dir = tempfile::tempdir().unwrap();
        let content = "---\nid: 3\ncompleted: true\n---\nDone.\n";
        let path = write_task_file(dir.path(), "03.md", content);

        // Should succeed (no-op)
        mark_completed(&path).unwrap();

        let task = load_task(&path).unwrap();
        assert!(task.completed);
    }

    #[test]
    fn mark_completed_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.md");
        assert!(mark_completed(&path).is_err());
    }

    #[test]
    fn mark_completed_no_completed_field() {
        let dir = tempfile::tempdir().unwrap();
        let content = "---\nid: 3\n---\nBody.\n";
        let path = write_task_file(dir.path(), "03.md", content);

        // No `completed` field at all — should error
        assert!(mark_completed(&path).is_err());
    }

    // --- find_ready_tasks tests ---

    fn ready(
        tasks: &[Task],
        steps: &[u32],
        completed: &[u32],
        failed: &[u32],
        in_flight: &[u32],
    ) -> Vec<TaskId> {
        let steps: Vec<TaskId> = steps.iter().map(|&id| TaskId(id)).collect();
        let completed: HashSet<TaskId> = completed.iter().map(|&id| TaskId(id)).collect();
        let failed: HashSet<TaskId> = failed.iter().map(|&id| TaskId(id)).collect();
        let in_flight: HashSet<TaskId> = in_flight.iter().map(|&id| TaskId(id)).collect();
        find_ready_tasks(tasks, &steps, &completed, &failed, &in_flight)
    }

    #[test]
    fn ready_single_no_deps() {
        let tasks = vec![make_task(0, &[])];
        let result = ready(&tasks, &[0], &[], &[], &[]);
        assert_eq!(result, vec![TaskId(0)]);
    }

    #[test]
    fn ready_single_already_completed() {
        let tasks = vec![make_task(0, &[])];
        let result = ready(&tasks, &[0], &[0], &[], &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn ready_single_already_failed() {
        let tasks = vec![make_task(0, &[])];
        let result = ready(&tasks, &[0], &[], &[0], &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn ready_single_in_flight() {
        let tasks = vec![make_task(0, &[])];
        let result = ready(&tasks, &[0], &[], &[], &[0]);
        assert!(result.is_empty());
    }

    #[test]
    fn ready_linear_chain_nothing_done() {
        // 0 -> 1 -> 2
        let tasks = vec![make_task(0, &[]), make_task(1, &[0]), make_task(2, &[1])];
        let result = ready(&tasks, &[0, 1, 2], &[], &[], &[]);
        // Only task 0 is ready (no deps)
        assert_eq!(result, vec![TaskId(0)]);
    }

    #[test]
    fn ready_linear_chain_first_done() {
        let tasks = vec![make_task(0, &[]), make_task(1, &[0]), make_task(2, &[1])];
        let result = ready(&tasks, &[0, 1, 2], &[0], &[], &[]);
        assert_eq!(result, vec![TaskId(1)]);
    }

    #[test]
    fn ready_linear_chain_first_two_done() {
        let tasks = vec![make_task(0, &[]), make_task(1, &[0]), make_task(2, &[1])];
        let result = ready(&tasks, &[0, 1, 2], &[0, 1], &[], &[]);
        assert_eq!(result, vec![TaskId(2)]);
    }

    #[test]
    fn ready_diamond_root_done() {
        // 0 -> 1, 0 -> 2, 1+2 -> 3
        let tasks = vec![
            make_task(0, &[]),
            make_task(1, &[0]),
            make_task(2, &[0]),
            make_task(3, &[1, 2]),
        ];
        let result = ready(&tasks, &[0, 1, 2, 3], &[0], &[], &[]);
        assert_eq!(result, vec![TaskId(1), TaskId(2)]);
    }

    #[test]
    fn ready_diamond_one_branch_done() {
        let tasks = vec![
            make_task(0, &[]),
            make_task(1, &[0]),
            make_task(2, &[0]),
            make_task(3, &[1, 2]),
        ];
        // Task 0 and 1 completed, 2 still in flight
        let result = ready(&tasks, &[0, 1, 2, 3], &[0, 1], &[], &[2]);
        // Task 3 can't run (dep 2 not completed)
        assert!(result.is_empty());
    }

    #[test]
    fn ready_diamond_both_branches_done() {
        let tasks = vec![
            make_task(0, &[]),
            make_task(1, &[0]),
            make_task(2, &[0]),
            make_task(3, &[1, 2]),
        ];
        let result = ready(&tasks, &[0, 1, 2, 3], &[0, 1, 2], &[], &[]);
        assert_eq!(result, vec![TaskId(3)]);
    }

    #[test]
    fn ready_wide_independent() {
        // 5 independent tasks, none started
        let tasks = vec![
            make_task(0, &[]),
            make_task(1, &[]),
            make_task(2, &[]),
            make_task(3, &[]),
            make_task(4, &[]),
        ];
        let result = ready(&tasks, &[0, 1, 2, 3, 4], &[], &[], &[]);
        assert_eq!(
            result,
            vec![TaskId(0), TaskId(1), TaskId(2), TaskId(3), TaskId(4)]
        );
    }

    #[test]
    fn ready_wide_some_in_flight() {
        let tasks = vec![
            make_task(0, &[]),
            make_task(1, &[]),
            make_task(2, &[]),
            make_task(3, &[]),
        ];
        let result = ready(&tasks, &[0, 1, 2, 3], &[], &[], &[0, 2]);
        assert_eq!(result, vec![TaskId(1), TaskId(3)]);
    }

    #[test]
    fn ready_all_completed() {
        let tasks = vec![make_task(0, &[]), make_task(1, &[0]), make_task(2, &[1])];
        let result = ready(&tasks, &[0, 1, 2], &[0, 1, 2], &[], &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn ready_all_failed() {
        let tasks = vec![make_task(0, &[]), make_task(1, &[]), make_task(2, &[])];
        let result = ready(&tasks, &[0, 1, 2], &[], &[0, 1, 2], &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn ready_failed_dep_blocks_dependent() {
        let tasks = vec![make_task(0, &[]), make_task(1, &[0]), make_task(2, &[])];
        // Task 0 failed, so task 1 should not be ready (dep 0 not in completed)
        // Task 2 is independent and ready
        let result = ready(&tasks, &[0, 1, 2], &[], &[0], &[]);
        assert_eq!(result, vec![TaskId(2)]);
    }

    #[test]
    fn ready_mixed_completed_failed_inflight() {
        // 0(done) -> 1(in_flight), 2(failed), 3(ready), 4(blocked by 2)
        let tasks = vec![
            make_task(0, &[]),
            make_task(1, &[0]),
            make_task(2, &[]),
            make_task(3, &[]),
            make_task(4, &[2]),
        ];
        let result = ready(&tasks, &[0, 1, 2, 3, 4], &[0], &[2], &[1]);
        // Task 3 is the only ready one
        assert_eq!(result, vec![TaskId(3)]);
    }

    #[test]
    fn ready_preserves_step_order() {
        let tasks = vec![make_task(0, &[]), make_task(1, &[]), make_task(2, &[])];
        // Steps in reverse order
        let result = ready(&tasks, &[2, 1, 0], &[], &[], &[]);
        assert_eq!(result, vec![TaskId(2), TaskId(1), TaskId(0)]);
    }

    #[test]
    fn ready_empty_steps() {
        let tasks = vec![make_task(0, &[])];
        let result = ready(&tasks, &[], &[], &[], &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn ready_empty_tasks() {
        let tasks: Vec<Task> = vec![];
        let result = ready(&tasks, &[0], &[], &[], &[]);
        // Task 0 not in task_map, so not ready
        assert!(result.is_empty());
    }
}
