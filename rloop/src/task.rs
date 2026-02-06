use std::path::{Path, PathBuf};

use crate::model::Model;

/// A task loaded from a markdown file with YAML frontmatter.
pub struct Task {
    pub id: String,
    pub model: Model,
    pub depends_on: Vec<String>,
    pub verification: Option<Vec<String>>,
    pub completed: bool,
    pub description: String,
    pub source_path: PathBuf,
}

/// Ordered execution plan from DAG resolution.
pub struct ExecutionPlan {
    pub target: String,
    pub steps: Vec<String>,
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
pub fn resolve_dag(_tasks: &[Task], _target: &str) -> anyhow::Result<ExecutionPlan> {
    todo!()
}
