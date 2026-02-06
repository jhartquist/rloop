use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
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
pub struct Task {
    pub id: TaskId,
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
    pub target: TaskId,
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
    let rest = trimmed.strip_prefix("---").context("no opening --- delimiter")?;

    // Find the closing ---
    let closing = rest
        .find("\n---")
        .context("no closing --- delimiter")?;

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
pub fn load_task(path: &Path) -> anyhow::Result<Task> {
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
pub fn load_all_tasks(dir: &Path) -> anyhow::Result<Vec<Task>> {
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

    let mut colors: HashMap<TaskId, Color> = task_map.keys().map(|&id| (id, Color::White)).collect();
    let mut order: Vec<TaskId> = Vec::new();

    dfs(target, &task_map, &mut colors, &mut order)?;

    Ok(ExecutionPlan {
        target,
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
    let mut colors: HashMap<TaskId, Color> = task_map.keys().map(|&id| (id, Color::White)).collect();
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
        target: TaskId(u32::MAX), // sentinel for "all tasks"
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
        assert!(matches!(task.model, Model::Sonnet)); // default
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
        assert!(matches!(task.model, Model::Opus));
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
        assert_eq!(plan.target, TaskId(0));
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
        let tasks = vec![
            make_task(0, &[2]),
            make_task(1, &[0]),
            make_task(2, &[1]),
        ];
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
        let tasks = vec![
            make_task(0, &[]),
            make_task(1, &[0]),
            make_task(2, &[1]),
        ];
        let plan = resolve_all_dag(&tasks).unwrap();
        assert_eq!(plan.steps, vec![TaskId(0), TaskId(1), TaskId(2)]);
    }

    #[test]
    fn dag_all_skips_completed() {
        let tasks = vec![
            Task { completed: true, ..make_task(0, &[]) },
            make_task(1, &[0]),
            make_task(2, &[1]),
        ];
        let plan = resolve_all_dag(&tasks).unwrap();
        assert_eq!(plan.steps, vec![TaskId(1), TaskId(2)]);
    }

    #[test]
    fn dag_all_empty_when_all_done() {
        let tasks = vec![
            Task { completed: true, ..make_task(0, &[]) },
            Task { completed: true, ..make_task(1, &[0]) },
        ];
        let plan = resolve_all_dag(&tasks).unwrap();
        assert!(plan.steps.is_empty());
    }
}
