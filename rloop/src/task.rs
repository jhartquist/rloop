use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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

/// Load a single task from a markdown file.
pub fn load_task(_path: &Path) -> anyhow::Result<Task> {
    todo!()
}

/// Load all tasks from a directory of markdown files.
pub fn load_all_tasks(_dir: &Path) -> anyhow::Result<Vec<Task>> {
    todo!()
}

/// Resolve the dependency DAG for a target task, returning execution order.
pub fn resolve_dag(_tasks: &[Task], _target: TaskId) -> anyhow::Result<ExecutionPlan> {
    todo!()
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
}
