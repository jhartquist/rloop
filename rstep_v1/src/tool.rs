use strum::{Display, EnumIter, EnumString};

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
