use std::collections::HashSet;

use strum::{Display, EnumIter, IntoEnumIterator, EnumString};

#[derive(Copy, Clone, Debug, Display, EnumIter, EnumString, PartialEq, Eq, Hash)]
pub enum BuiltinTool {
    Task,
    TaskOutput,
    Bash,
    Glob,
    Grep,
    ExitPlanMode,
    Read,
    Write,
    Edit,
    NotebookEdit,
    WebFetch,
    TodoWrite,
    WebSearch,
    TaskStop,
    AskUserQuestion,
    Skill,
    EnterPlanMode,
    ToolSearch,
}

/// Compute the disallowed builtin tools by taking the set difference:
/// `all_builtins - allowed_builtins`.
///
/// Returns the disallowed set.
pub(crate) fn compute_disallowed_builtins(
    allowed: &HashSet<BuiltinTool>,
) -> HashSet<BuiltinTool> {
    BuiltinTool::iter()
        .collect::<HashSet<_>>()
        .difference(allowed)
        .copied()
        .collect()
}
