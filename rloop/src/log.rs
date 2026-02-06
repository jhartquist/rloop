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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionId;
    use crate::task::TaskId;
    use serde_json::json;

    #[test]
    fn serialize_session_started() {
        let event = Event::SessionStarted {
            session_id: SessionId("abc-123".into()),
            target: TaskId(5),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"SessionStarted"#));
        assert!(json.contains(r#""session_id":"abc-123"#));
        assert!(json.contains(r#""target":5"#));
    }

    #[test]
    fn serialize_all_variants() {
        let events = vec![
            Event::SessionStarted { session_id: SessionId("s1".into()), target: TaskId(0) },
            Event::TaskStarted { task_id: TaskId(1) },
            Event::WorktreeCreated { task_id: TaskId(2), path: PathBuf::from("/tmp/wt") },
            Event::PromptSent { task_id: TaskId(3) },
            Event::VerificationRan { task_id: TaskId(4), command: "cargo test".into(), passed: true, output: "ok".into() },
            Event::TaskCompleted { task_id: TaskId(5), summary: "done".into() },
            Event::TaskFailed { task_id: TaskId(6), reason: "fail".into() },
            Event::WorktreeMerged { task_id: TaskId(7), into_branch: "main".into() },
            Event::WorktreeCleanedUp { task_id: TaskId(8) },
            Event::SessionComplete { branch: "rloop/05".into() },
            Event::SessionAutoMerged { branch: "rloop/05".into(), into_branch: "main".into() },
            Event::SessionCancelled,
            Event::AssistantMessage { content: "hello".into() },
            Event::ToolCall { name: "read".into(), input: json!({"path": "/foo"}) },
            Event::ToolResult { name: "read".into(), output: json!("contents") },
        ];
        for event in &events {
            let json = serde_json::to_string(event);
            assert!(json.is_ok(), "Failed to serialize: {json:?}");
        }
    }

    #[test]
    fn serialize_task_id_in_event() {
        let event = Event::TaskStarted { task_id: TaskId(42) };
        let json = serde_json::to_string(&event).unwrap();
        // TaskId serializes as number (42), not string ("42")
        assert!(json.contains(r#""task_id":42"#));
    }

    #[test]
    fn serialize_tool_call_nested_json() {
        let event = Event::ToolCall {
            name: "bash".into(),
            input: json!({"command": "ls", "options": {"all": true}}),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("options"));
    }

    #[test]
    fn serialize_special_chars() {
        let event = Event::AssistantMessage {
            content: "line1\nline2\t\"quoted\"".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        // JSON escapes newlines and quotes
        assert!(json.contains("\\n"));
        assert!(json.contains("\\\"quoted\\\""));
    }

    #[test]
    fn logger_new() {
        let dir = tempfile::tempdir().unwrap();
        let _logger = SessionLogger::new(dir.path(), SessionId("test-session".into())).unwrap();
    }

    #[test]
    fn logger_log_writes_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let mut logger = SessionLogger::new(dir.path(), SessionId("test".into())).unwrap();
        let event = Event::SessionCancelled;
        logger.log(&event).unwrap();
    }

    #[test]
    fn logger_multiple_events_append() {
        let dir = tempfile::tempdir().unwrap();
        let mut logger = SessionLogger::new(dir.path(), SessionId("test".into())).unwrap();
        logger.log(&Event::TaskStarted { task_id: TaskId(0) }).unwrap();
        logger.log(&Event::TaskCompleted { task_id: TaskId(0), summary: "done".into() }).unwrap();
    }
}
