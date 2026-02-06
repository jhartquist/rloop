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
