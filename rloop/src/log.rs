use std::path::{Path, PathBuf};

use serde::Serialize;

/// All events that can occur during a session.
#[derive(Serialize)]
#[serde(tag = "type")]
pub enum Event {
    // rloop lifecycle
    SessionStarted {
        session_id: String,
        target: String,
    },
    TaskStarted {
        task_id: String,
    },
    WorktreeCreated {
        task_id: String,
        path: PathBuf,
    },
    PromptSent {
        task_id: String,
    },
    VerificationRan {
        task_id: String,
        command: String,
        passed: bool,
        output: String,
    },
    TaskCompleted {
        task_id: String,
        summary: String,
    },
    TaskFailed {
        task_id: String,
        reason: String,
    },
    WorktreeMerged {
        task_id: String,
        into_branch: String,
    },
    WorktreeCleanedUp {
        task_id: String,
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
    TokenUsage {
        input_tokens: u64,
        output_tokens: u64,
        cache_read: u64,
        cache_creation: u64,
    },
    ContextUsage {
        percentage: f32,
    },
}

/// Logs session events to JSONL files and prints to terminal.
pub struct SessionLogger {
    _session_dir: PathBuf,
    _session_id: String,
}

impl SessionLogger {
    /// Create a new logger for a session.
    pub fn new(_session_dir: &Path, _session_id: &str) -> anyhow::Result<Self> {
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
