use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use colored::Colorize;
use serde::{Deserialize, Serialize};

use crate::session::SessionId;
use crate::task::TaskId;

/// All events that can occur during a session.
#[derive(Clone, Debug, Serialize, Deserialize)]
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
    writer: BufWriter<File>,
    _session_id: SessionId,
}

impl SessionLogger {
    /// Create a new logger for a session.
    ///
    /// Opens (or creates) `{session_dir}/{session_id}.jsonl` for appending.
    ///
    /// # Errors
    /// Returns an error if directory creation or file opening fails.
    pub fn new(session_dir: impl AsRef<Path>, session_id: SessionId) -> anyhow::Result<Self> {
        let session_dir = session_dir.as_ref();
        std::fs::create_dir_all(session_dir)?;
        let path = session_dir.join(format!("{session_id}.jsonl"));
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            writer: BufWriter::new(file),
            _session_id: session_id,
        })
    }

    /// Write an event to the JSONL log file.
    ///
    /// # Errors
    /// Returns an error if JSON serialization or file writing fails.
    pub fn log(&mut self, event: &Event) -> anyhow::Result<()> {
        let line = serde_json::to_string(event)?;
        writeln!(self.writer, "{line}")?;
        self.writer.flush()?;
        Ok(())
    }

    /// Pretty-print an event to the terminal.
    #[allow(clippy::unused_self)]
    pub fn print(&self, event: &Event) {
        match event {
            Event::SessionStarted { session_id, target } => {
                eprintln!(
                    "{} session {} targeting task {}",
                    "▶".green(),
                    session_id,
                    target
                );
            }
            Event::TaskStarted { task_id } => {
                eprintln!("{} task {task_id}", "→".blue());
            }
            Event::TaskCompleted { task_id, summary } => {
                eprintln!("{} task {task_id} — {summary}", "✓".green());
            }
            Event::TaskFailed { task_id, reason } => {
                eprintln!("{} task {task_id} — {reason}", "✗".red());
            }
            Event::VerificationRan {
                task_id,
                command,
                passed,
                output,
            } => {
                let icon = if *passed { "✓".green() } else { "✗".red() };
                let status = if *passed { "passed" } else { "failed" };
                eprintln!("{icon} verify task {task_id} `{command}`: {status}");
                if !passed {
                    eprintln!("  {output}");
                }
            }
            Event::WorktreeCreated { task_id, path } => {
                eprintln!("  worktree {task_id} → {}", path.display());
            }
            Event::WorktreeMerged {
                task_id,
                into_branch,
            } => {
                eprintln!("  merged {task_id} → {into_branch}");
            }
            Event::WorktreeCleanedUp { task_id } => {
                eprintln!("  cleaned up {task_id}");
            }
            Event::SessionComplete { branch } => {
                eprintln!("{} session complete → branch {branch}", "■".green());
            }
            Event::SessionAutoMerged {
                branch,
                into_branch,
            } => {
                eprintln!("{} auto-merged {branch} → {into_branch}", "■".green());
            }
            Event::SessionCancelled => {
                eprintln!("{} session cancelled", "■".yellow());
            }
            Event::AssistantMessage { content } => {
                eprintln!("{content}");
            }
            Event::PromptSent { task_id } => {
                eprintln!("  prompt → task {task_id}");
            }
            Event::ToolCall { name, .. } => {
                eprintln!("  tool: {name}");
            }
            Event::ToolResult { name, .. } => {
                eprintln!("  result: {name}");
            }
        }
    }
}

/// Rich terminal printer for Claude stream events.
///
/// Formats assistant messages, tool calls, and tool results with colored labels
/// and relative file paths. Tracks pending tool calls to correlate with results.
pub struct StreamPrinter {
    project_dir: PathBuf,
    pending_tools: HashMap<String, (String, serde_json::Value)>,
}

impl StreamPrinter {
    pub fn new(project_dir: impl Into<PathBuf>) -> Self {
        let project_dir = project_dir.into();
        let project_dir = project_dir.canonicalize().unwrap_or(project_dir);
        Self {
            project_dir,
            pending_tools: HashMap::new(),
        }
    }

    /// Register a tool call so its result can be correlated later.
    pub fn register_tool_call(&mut self, id: &str, name: &str, input: &serde_json::Value) {
        self.pending_tools
            .insert(id.to_string(), (name.to_string(), input.clone()));
    }

    /// Look up and remove a pending tool call by its ID.
    pub fn take_pending_tool(&mut self, id: &str) -> Option<(String, serde_json::Value)> {
        self.pending_tools.remove(id)
    }

    /// Pretty-print a stream event to the terminal.
    pub fn print(&self, event: &Event) {
        match event {
            Event::AssistantMessage { content } => {
                let label = "  ASST".cyan().bold();
                let pad = "      ";
                for (i, line) in content.lines().enumerate() {
                    if i == 0 {
                        eprintln!("{label}  {line}");
                    } else {
                        eprintln!("{pad}  {line}");
                    }
                }
            }
            Event::ToolCall { name, input } => {
                self.print_tool_call(name, input);
            }
            Event::ToolResult { name, output } => {
                self.print_tool_result(name, output);
            }
            _ => {}
        }
    }

    fn print_tool_call(&self, name: &str, input: &serde_json::Value) {
        let label = "  CALL".yellow().bold();
        let pad = "      ";
        match name {
            "Read" => {
                let path = input
                    .get("file_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                eprintln!("{label}  {} {}", "Read".dimmed(), self.rel(path).dimmed());
            }
            "Edit" => {
                let path = input
                    .get("file_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                eprintln!("{label}  {} {}", "Edit".dimmed(), self.rel(path).dimmed());
                if let Some(old) = input.get("old_string").and_then(|v| v.as_str()) {
                    for line in old.lines() {
                        eprintln!("{pad}  {} {}", "-".red(), line.dimmed());
                    }
                }
                if let Some(new) = input.get("new_string").and_then(|v| v.as_str()) {
                    for line in new.lines() {
                        eprintln!("{pad}  {} {}", "+".green(), line.dimmed());
                    }
                }
            }
            "Write" => {
                let path = input
                    .get("file_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                eprintln!("{label}  {} {}", "Write".dimmed(), self.rel(path).dimmed());
                if let Some(content) = input.get("content").and_then(|v| v.as_str()) {
                    for line in content.lines() {
                        eprintln!("{pad}  {}", line.dimmed());
                    }
                }
            }
            "Bash" => {
                let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("?");
                eprintln!(
                    "{label}  {} {}",
                    "Bash".dimmed(),
                    self.shorten(cmd).dimmed()
                );
            }
            other => {
                let param = self.extract_key_param(input);
                if param.is_empty() {
                    eprintln!("{label}  {}", other.dimmed());
                } else {
                    eprintln!("{label}  {} {}", other.dimmed(), param.dimmed());
                }
            }
        }
    }

    fn print_tool_result(&self, name: &str, output: &serde_json::Value) {
        let label = "   RES".yellow().bold();

        // Write/Edit: details already shown in CALL, just confirm
        if matches!(name, "Write" | "Edit") {
            eprintln!("{label}  {}", format!("{name} ok").dimmed());
            return;
        }

        // Read result: try to show file path and line count
        if name == "Read"
            && let Some(file) = output.get("file")
        {
            let path = file.get("filePath").and_then(|v| v.as_str()).unwrap_or("?");
            let num_lines = file
                .get("numLines")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            eprintln!(
                "{label}  {}",
                format!("{} ({num_lines} lines)", self.rel(path)).dimmed()
            );
            return;
        }

        // Try to extract text from various result shapes
        if let Some(text) = output
            .as_str()
            .or_else(|| output.get("text").and_then(|v| v.as_str()))
            .or_else(|| output.get("content").and_then(|v| v.as_str()))
            .or_else(|| output.get("stdout").and_then(|v| v.as_str()))
            .or_else(|| output.get("output").and_then(|v| v.as_str()))
        {
            eprintln!("{label}  {}", name.dimmed());
            for line in text.lines() {
                eprintln!("        {}", self.shorten(line).dimmed());
            }
            return;
        }

        eprintln!("{label}  {}", format!("{name} ok").dimmed());
    }

    /// Strip `project_dir` prefix from a path string.
    fn rel(&self, path: &str) -> String {
        let prefix = self.project_dir.to_string_lossy();
        path.strip_prefix(prefix.as_ref())
            .map_or(path, |p| p.strip_prefix('/').unwrap_or(p))
            .to_string()
    }

    /// Replace `project_dir` prefix anywhere in a string.
    fn shorten(&self, text: &str) -> String {
        let prefix = format!("{}/", self.project_dir.to_string_lossy());
        text.replace(&prefix, "")
    }

    fn extract_key_param(&self, input: &serde_json::Value) -> String {
        if let Some(obj) = input.as_object() {
            for key in ["command", "pattern", "query", "file_path", "path"] {
                if let Some(val) = obj.get(key).and_then(|v| v.as_str()) {
                    return self.rel(val);
                }
            }
        }
        String::new()
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
            Event::SessionStarted {
                session_id: SessionId("s1".into()),
                target: TaskId(0),
            },
            Event::TaskStarted { task_id: TaskId(1) },
            Event::WorktreeCreated {
                task_id: TaskId(2),
                path: PathBuf::from("/tmp/wt"),
            },
            Event::PromptSent { task_id: TaskId(3) },
            Event::VerificationRan {
                task_id: TaskId(4),
                command: "cargo test".into(),
                passed: true,
                output: "ok".into(),
            },
            Event::TaskCompleted {
                task_id: TaskId(5),
                summary: "done".into(),
            },
            Event::TaskFailed {
                task_id: TaskId(6),
                reason: "fail".into(),
            },
            Event::WorktreeMerged {
                task_id: TaskId(7),
                into_branch: "main".into(),
            },
            Event::WorktreeCleanedUp { task_id: TaskId(8) },
            Event::SessionComplete {
                branch: "rloop/05".into(),
            },
            Event::SessionAutoMerged {
                branch: "rloop/05".into(),
                into_branch: "main".into(),
            },
            Event::SessionCancelled,
            Event::AssistantMessage {
                content: "hello".into(),
            },
            Event::ToolCall {
                name: "read".into(),
                input: json!({"path": "/foo"}),
            },
            Event::ToolResult {
                name: "read".into(),
                output: json!("contents"),
            },
        ];
        for event in &events {
            let json = serde_json::to_string(event);
            assert!(json.is_ok(), "Failed to serialize: {json:?}");
        }
    }

    #[test]
    fn serialize_task_id_in_event() {
        let event = Event::TaskStarted {
            task_id: TaskId(42),
        };
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
        logger
            .log(&Event::TaskStarted { task_id: TaskId(0) })
            .unwrap();
        logger
            .log(&Event::TaskCompleted {
                task_id: TaskId(0),
                summary: "done".into(),
            })
            .unwrap();
    }
}
