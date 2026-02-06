use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::session::SessionId;
use crate::task::TaskId;

/// All events that can occur during a session.
#[derive(Serialize)]
#[serde(tag = "type")]
pub enum Event {
    // rloop lifecycle
    SessionStarted {
        session_id: SessionId,
        target: TaskId,
    },
    TaskStarted {
        task_id: TaskId,
    },
    WorktreeCreated {
        task_id: TaskId,
        path: PathBuf,
    },
    PromptSent {
        task_id: TaskId,
    },
    VerificationRan {
        task_id: TaskId,
        command: String,
        passed: bool,
        output: String,
    },
    TaskCompleted {
        task_id: TaskId,
        summary: String,
    },
    TaskFailed {
        task_id: TaskId,
        reason: String,
    },
    WorktreeMerged {
        task_id: TaskId,
        into_branch: String,
    },
    WorktreeCleanedUp {
        task_id: TaskId,
    },
    SessionComplete {
        branch: String,
    },
    SessionAutoMerged {
        branch: String,
        into_branch: String,
    },
    SessionCancelled,

    // Claude stream events
    AssistantMessage {
        content: String,
    },
    ToolCall {
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        name: String,
        output: serde_json::Value,
    },
}

/// Logs session events to JSONL files and prints to terminal.
pub struct SessionLogger {
    _session_id: SessionId,
}

impl SessionLogger {
    /// Create a new logger for a session.
    pub fn new(_session_dir: &Path, _session_id: SessionId) -> anyhow::Result<Self> {
        todo!()
    }

    /// Write an event to the JSONL log file.
    pub fn log(&mut self, _event: &Event) -> anyhow::Result<()> {
        todo!()
    }

    /// Pretty-print an event to the terminal.
    pub fn print(&self, _event: &Event) {
        todo!()
    }
}
