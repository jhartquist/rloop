use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

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
    /// Rich session banner printed at session start.
    SessionBanner {
        session_id: SessionId,
        datetime: String,
        target: TaskId,
        parent_branch: String,
        session_branch: String,
    },
    /// Step start info printed when a step begins executing.
    StepStarted {
        task_id: TaskId,
        attempt: u32,
        model_id: String,
        cc_session_id: String,
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
    /// Per-message token usage from the Claude API.
    TokenUsage {
        input_tokens: u64,
        output_tokens: u64,
        cache_read_input_tokens: u64,
        cache_creation_input_tokens: u64,
    },
    /// Agent's current context window utilization as a percentage.
    ContextUsage {
        percentage: f32,
        recommendation: String,
    },

    // Metrics summaries
    /// One-line summary after a step completes.
    StepSummary {
        task_id: TaskId,
        step_number: u32,
        max_steps: u32,
        #[serde(with = "crate::step::duration_serde")]
        duration: std::time::Duration,
        cost_usd: f64,
        input_tokens: u64,
        output_tokens: u64,
    },
    /// One-line summary after a task completes (aggregating all steps).
    TaskSummary {
        task_id: TaskId,
        success: bool,
        #[serde(with = "crate::step::duration_serde")]
        duration: std::time::Duration,
        cost_usd: f64,
        input_tokens: u64,
        output_tokens: u64,
        steps: u32,
    },
    /// Session-level totals printed at end.
    SessionSummary {
        session_id: SessionId,
        #[serde(with = "crate::step::duration_serde")]
        duration: std::time::Duration,
        cost_usd: f64,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_creation_tokens: u64,
    },
}

/// Format a duration as `Xm Ys` or `Xs` for short durations.
fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

/// Format elapsed time with `+` prefix and right-align to 8 characters.
fn format_elapsed(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    let time_str = if secs >= 60 {
        format!("+{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("+{secs}s")
    };
    format!("{time_str:>8}")
}

/// Format a token count with `k` suffix for thousands.
fn format_tokens(n: u64) -> String {
    if n >= 1000 {
        #[allow(clippy::cast_precision_loss)]
        let k = n as f64 / 1000.0;
        format!("{k:.0}k")
    } else {
        n.to_string()
    }
}

/// Logs session events to JSONL files and prints to terminal.
pub struct SessionLogger {
    writer: BufWriter<File>,
    _session_id: SessionId,
    start_time: Instant,
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
            start_time: Instant::now(),
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
    #[allow(clippy::too_many_lines)]
    pub fn print(&self, event: &Event) {
        let elapsed = format_elapsed(self.start_time.elapsed());
        match event {
            Event::SessionStarted { session_id, target } => {
                eprintln!(
                    "{elapsed}  {} session {} targeting task {}",
                    "▶".green(),
                    session_id,
                    target
                );
            }
            Event::SessionBanner {
                session_id,
                datetime,
                target,
                parent_branch,
                session_branch,
            } => {
                eprintln!("{elapsed}  {} Session {session_id}", "▶".green());
                eprintln!("           Date:   {datetime}");
                eprintln!("           Target: task {target:02}");
                eprintln!("           Branch: {parent_branch} → {session_branch}");
            }
            Event::StepStarted {
                task_id,
                attempt,
                model_id,
                cc_session_id,
            } => {
                eprintln!(
                    "{elapsed}  {} Task {task_id:02}  step {attempt}  model: {model_id}  cc: {cc_session_id}",
                    "→".blue(),
                );
            }
            Event::TaskStarted { task_id } => {
                eprintln!("{elapsed}  {} task {task_id}", "→".blue());
            }
            Event::TaskCompleted { task_id, summary } => {
                eprintln!("{elapsed}  {} task {task_id} — {summary}", "✓".green());
            }
            Event::TaskFailed { task_id, reason } => {
                eprintln!("{elapsed}  {} task {task_id} — {reason}", "✗".red());
            }
            Event::VerificationRan {
                task_id,
                command,
                passed,
                output,
            } => {
                let icon = if *passed { "✓".green() } else { "✗".red() };
                let status = if *passed { "passed" } else { "failed" };
                eprintln!("{elapsed}  {icon} verify task {task_id} `{command}`: {status}");
                if !passed {
                    eprintln!("           {output}");
                }
            }
            Event::WorktreeCreated { task_id, path } => {
                eprintln!("{elapsed}    worktree {task_id} → {}", path.display());
            }
            Event::WorktreeMerged {
                task_id,
                into_branch,
            } => {
                eprintln!("{elapsed}    merged {task_id} → {into_branch}");
            }
            Event::WorktreeCleanedUp { task_id } => {
                eprintln!("{elapsed}    cleaned up {task_id}");
            }
            Event::SessionComplete { branch } => {
                eprintln!("{elapsed}  {} session complete → branch {branch}", "■".green());
            }
            Event::SessionAutoMerged {
                branch,
                into_branch,
            } => {
                eprintln!("{elapsed}  {} auto-merged {branch} → {into_branch}", "■".green());
            }
            Event::SessionCancelled => {
                eprintln!("{elapsed}  {} session cancelled", "■".yellow());
            }
            Event::AssistantMessage { content } => {
                eprintln!("{elapsed}  {content}");
            }
            Event::PromptSent { task_id } => {
                eprintln!("{elapsed}    prompt → task {task_id}");
            }
            Event::ToolCall { name, .. } => {
                eprintln!("{elapsed}    tool: {name}");
            }
            Event::ToolResult { name, .. } => {
                eprintln!("{elapsed}    result: {name}");
            }
            Event::TokenUsage {
                input_tokens,
                output_tokens,
                ..
            } => {
                eprintln!("{elapsed}    tokens: {input_tokens} in / {output_tokens} out");
            }
            Event::ContextUsage { percentage, .. } => {
                eprintln!("{elapsed}    context: {percentage:.1}%");
            }
            Event::StepSummary {
                task_id,
                step_number,
                max_steps,
                duration,
                cost_usd,
                input_tokens,
                output_tokens,
            } => {
                eprintln!(
                    "{elapsed}    {} step {step_number}/{max_steps}  {}  ${cost_usd:.2}  tokens: {} in / {} out",
                    format!("task {task_id}").dimmed(),
                    format_duration(*duration),
                    format_tokens(*input_tokens),
                    format_tokens(*output_tokens),
                );
            }
            Event::TaskSummary {
                task_id,
                success,
                duration,
                cost_usd,
                input_tokens,
                output_tokens,
                steps,
            } => {
                let status = if *success {
                    "done".green().to_string()
                } else {
                    "failed".red().to_string()
                };
                eprintln!(
                    "{elapsed}  {} task {task_id:02}  {status}  {}  ${cost_usd:.2}  tokens: {} in / {} out  ({steps} step{})",
                    "─".dimmed(),
                    format_duration(*duration),
                    format_tokens(*input_tokens),
                    format_tokens(*output_tokens),
                    if *steps == 1 { "" } else { "s" },
                );
            }
            Event::SessionSummary {
                session_id,
                duration,
                cost_usd,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
            } => {
                eprintln!(
                    "{elapsed}    Duration: {}  Cost: ${cost_usd:.2}",
                    format_duration(*duration),
                );
                eprintln!(
                    "             Tokens: {} in / {} out / {} cache-read / {} cache-create",
                    format_tokens(*input_tokens),
                    format_tokens(*output_tokens),
                    format_tokens(*cache_read_tokens),
                    format_tokens(*cache_creation_tokens),
                );
                let _ = session_id; // used in serialization only
            }
        }
    }
}

/// Load events from a JSONL session log file.
///
/// Each line is parsed as a JSON-encoded `Event`. Lines that fail to parse
/// are skipped with a warning logged to stderr (including line number and content).
///
/// # Errors
/// Returns an error if the file cannot be opened or read.
pub fn load_events(path: impl AsRef<Path>) -> anyhow::Result<Vec<Event>> {
    let path = path.as_ref();
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut events = Vec::new();

    for (line_number, line_result) in reader.lines().enumerate() {
        let line_number = line_number + 1; // 1-based
        let line = match line_result {
            Ok(l) => l,
            Err(e) => {
                eprintln!(
                    "warning: failed to read line {line_number} in {}: {e}",
                    path.display()
                );
                continue;
            }
        };

        if line.trim().is_empty() {
            continue;
        }

        match serde_json::from_str::<Event>(&line) {
            Ok(event) => events.push(event),
            Err(e) => {
                eprintln!(
                    "warning: skipping corrupt line {line_number} in {}: {e} — content: {line}",
                    path.display()
                );
            }
        }
    }

    Ok(events)
}

/// Write data to a file, creating a `.bak` backup of any existing file first.
///
/// If the target file exists, it is copied to `{path}.bak` before being overwritten.
/// The write is performed atomically via a temporary file and rename to reduce the
/// risk of data loss from partial writes.
///
/// # Errors
/// Returns an error if backup creation, temp file writing, or rename fails.
pub fn write_with_backup(path: impl AsRef<Path>, data: &[u8]) -> anyhow::Result<()> {
    let path = path.as_ref();
    let backup_path = path.with_extension(path.extension().map_or_else(
        || "bak".to_string(),
        |ext| format!("{}.bak", ext.to_string_lossy()),
    ));

    // Back up existing file
    if path.exists() {
        fs::copy(path, &backup_path).map_err(|e| {
            anyhow::anyhow!(
                "failed to create backup {} -> {}: {e}",
                path.display(),
                backup_path.display()
            )
        })?;
    }

    // Write to a temporary file in the same directory, then rename for atomicity
    let parent = path.parent().unwrap_or(Path::new("."));
    let tmp_path = parent.join(format!(
        ".{}.tmp",
        path.file_name()
            .map_or_else(|| "file".to_string(), |n| n.to_string_lossy().to_string())
    ));

    fs::write(&tmp_path, data)
        .map_err(|e| anyhow::anyhow!("failed to write temp file {}: {e}", tmp_path.display()))?;

    fs::rename(&tmp_path, path).map_err(|e| {
        // Clean up temp file on rename failure
        if let Err(cleanup_err) = fs::remove_file(&tmp_path) {
            eprintln!(
                "warning: failed to clean up temp file {}: {cleanup_err}",
                tmp_path.display()
            );
        }
        anyhow::anyhow!(
            "failed to rename {} -> {}: {e}",
            tmp_path.display(),
            path.display()
        )
    })?;

    Ok(())
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
            Event::TokenUsage {
                input_tokens,
                output_tokens,
                cache_read_input_tokens,
                cache_creation_input_tokens,
            } => {
                let label = " USAGE".dimmed();
                eprintln!(
                    "{label}  in={input_tokens} out={output_tokens} \
                     cache_read={cache_read_input_tokens} cache_create={cache_creation_input_tokens}"
                );
            }
            Event::ContextUsage {
                percentage,
                recommendation,
            } => {
                let label = "   CTX".dimmed();
                eprintln!("{label}  {percentage:.1}% — {recommendation}");
            }
            // Metric summaries are printed by SessionLogger, not StreamPrinter
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
        let events = all_event_variants();
        for event in &events {
            let json = serde_json::to_string(event);
            assert!(json.is_ok(), "Failed to serialize: {json:?}");
        }
    }

    fn all_event_variants() -> Vec<Event> {
        vec![
            Event::SessionStarted {
                session_id: SessionId("s1".into()),
                target: TaskId(0),
            },
            Event::SessionBanner {
                session_id: SessionId("20250206-143022".into()),
                datetime: "2025-02-06 14:30:22 PST".into(),
                target: TaskId(5),
                parent_branch: "main".into(),
                session_branch: "rloop-05".into(),
            },
            Event::StepStarted {
                task_id: TaskId(3),
                attempt: 1,
                model_id: "claude-sonnet-4-20250514".into(),
                cc_session_id: "abc-123".into(),
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
            Event::TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_read_input_tokens: 1000,
                cache_creation_input_tokens: 500,
            },
            Event::ContextUsage {
                percentage: 42.5,
                recommendation: "plenty of room".into(),
            },
            Event::StepSummary {
                task_id: TaskId(1),
                step_number: 1,
                max_steps: 3,
                duration: std::time::Duration::from_secs(222),
                cost_usd: 0.12,
                input_tokens: 45000,
                output_tokens: 12000,
            },
            Event::TaskSummary {
                task_id: TaskId(3),
                success: true,
                duration: std::time::Duration::from_secs(318),
                cost_usd: 0.25,
                input_tokens: 82000,
                output_tokens: 28000,
                steps: 2,
            },
            Event::SessionSummary {
                session_id: SessionId("20250206-143022".into()),
                duration: std::time::Duration::from_secs(765),
                cost_usd: 0.72,
                input_tokens: 245_000,
                output_tokens: 89000,
                cache_read_tokens: 12000,
                cache_creation_tokens: 5000,
            },
        ]
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

    #[test]
    fn roundtrip_token_usage() {
        let event = Event::TokenUsage {
            input_tokens: 3,
            output_tokens: 457,
            cache_read_input_tokens: 64996,
            cache_creation_input_tokens: 8280,
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: Event = serde_json::from_str(&json).unwrap();
        match deserialized {
            Event::TokenUsage {
                input_tokens,
                output_tokens,
                cache_read_input_tokens,
                cache_creation_input_tokens,
            } => {
                assert_eq!(input_tokens, 3);
                assert_eq!(output_tokens, 457);
                assert_eq!(cache_read_input_tokens, 64996);
                assert_eq!(cache_creation_input_tokens, 8280);
            }
            other => panic!("Expected TokenUsage, got {other:?}"),
        }
    }

    #[test]
    fn roundtrip_context_usage() {
        let event = Event::ContextUsage {
            percentage: 47.3,
            recommendation: "plenty of room".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: Event = serde_json::from_str(&json).unwrap();
        match deserialized {
            Event::ContextUsage {
                percentage,
                recommendation,
            } => {
                assert!((percentage - 47.3).abs() < f32::EPSILON);
                assert_eq!(recommendation, "plenty of room");
            }
            other => panic!("Expected ContextUsage, got {other:?}"),
        }
    }

    #[test]
    fn roundtrip_all_event_variants() {
        let events = all_event_variants();
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let roundtripped: Event = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&roundtripped).unwrap();
            assert_eq!(json, json2, "Roundtrip failed for {event:?}");
        }
    }

    #[test]
    fn token_usage_zero_values() {
        let event = Event::TokenUsage {
            input_tokens: 0,
            output_tokens: 0,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: Event = serde_json::from_str(&json).unwrap();
        match deserialized {
            Event::TokenUsage {
                input_tokens,
                output_tokens,
                cache_read_input_tokens,
                cache_creation_input_tokens,
            } => {
                assert_eq!(input_tokens, 0);
                assert_eq!(output_tokens, 0);
                assert_eq!(cache_read_input_tokens, 0);
                assert_eq!(cache_creation_input_tokens, 0);
            }
            other => panic!("Expected TokenUsage, got {other:?}"),
        }
    }

    #[test]
    fn token_usage_large_values() {
        let event = Event::TokenUsage {
            input_tokens: u64::MAX,
            output_tokens: u64::MAX,
            cache_read_input_tokens: u64::MAX,
            cache_creation_input_tokens: u64::MAX,
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: Event = serde_json::from_str(&json).unwrap();
        match deserialized {
            Event::TokenUsage {
                input_tokens,
                output_tokens,
                ..
            } => {
                assert_eq!(input_tokens, u64::MAX);
                assert_eq!(output_tokens, u64::MAX);
            }
            other => panic!("Expected TokenUsage, got {other:?}"),
        }
    }

    // --- load_events tests ---

    #[test]
    fn load_events_valid_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut logger = SessionLogger::new(dir.path(), SessionId("session".into())).unwrap();
        logger
            .log(&Event::TaskStarted { task_id: TaskId(0) })
            .unwrap();
        logger
            .log(&Event::TaskCompleted {
                task_id: TaskId(0),
                summary: "done".into(),
            })
            .unwrap();
        logger.log(&Event::SessionCancelled).unwrap();

        let events = load_events(&path).unwrap();
        assert_eq!(events.len(), 3);
        assert!(matches!(&events[0], Event::TaskStarted { task_id } if *task_id == TaskId(0)));
        assert!(matches!(&events[2], Event::SessionCancelled));
    }

    #[test]
    fn load_events_skips_corrupt_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mixed.jsonl");

        let valid = serde_json::to_string(&Event::SessionCancelled).unwrap();
        let content = format!("{valid}\nnot valid json\n{valid}\n");
        std::fs::write(&path, content).unwrap();

        let events = load_events(&path).unwrap();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn load_events_all_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("corrupt.jsonl");
        std::fs::write(&path, "garbage\n{bad json}\nmore garbage\n").unwrap();

        let events = load_events(&path).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn load_events_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.jsonl");
        std::fs::write(&path, "").unwrap();

        let events = load_events(&path).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn load_events_blank_lines_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blanks.jsonl");

        let valid = serde_json::to_string(&Event::SessionCancelled).unwrap();
        let content = format!("\n{valid}\n\n{valid}\n\n");
        std::fs::write(&path, content).unwrap();

        let events = load_events(&path).unwrap();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn load_events_missing_file() {
        let result = load_events("/nonexistent/path/session.jsonl");
        assert!(result.is_err());
    }

    #[test]
    fn load_events_partial_write_recovery() {
        // Simulates a crash mid-write: valid lines followed by a truncated JSON line
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("partial.jsonl");

        let valid = serde_json::to_string(&Event::TaskStarted { task_id: TaskId(1) }).unwrap();
        let truncated = r#"{"type":"TaskCompleted","task_id":2,"summ"#;
        let content = format!("{valid}\n{truncated}\n");
        std::fs::write(&path, content).unwrap();

        let events = load_events(&path).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], Event::TaskStarted { task_id } if *task_id == TaskId(1)));
    }

    // --- write_with_backup tests ---

    #[test]
    fn write_with_backup_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.json");

        write_with_backup(&path, b"hello").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
        // No backup should exist for a new file
        assert!(!dir.path().join("data.json.bak").exists());
    }

    #[test]
    fn write_with_backup_creates_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.json");

        std::fs::write(&path, "original").unwrap();
        write_with_backup(&path, b"updated").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "updated");
        let backup = dir.path().join("data.json.bak");
        assert!(backup.exists());
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "original");
    }

    #[test]
    fn write_with_backup_overwrites_previous_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.json");

        std::fs::write(&path, "v1").unwrap();
        write_with_backup(&path, b"v2").unwrap();
        write_with_backup(&path, b"v3").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v3");
        let backup = dir.path().join("data.json.bak");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "v2");
    }

    #[test]
    fn write_with_backup_no_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("TASKS");

        std::fs::write(&path, "original").unwrap();
        write_with_backup(&path, b"updated").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "updated");
        let backup = dir.path().join("TASKS.bak");
        assert!(backup.exists());
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "original");
    }

    #[test]
    fn write_with_backup_restores_from_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.json");

        // Write original
        std::fs::write(&path, "important data").unwrap();
        // Overwrite with backup
        write_with_backup(&path, b"new data").unwrap();

        // Simulate restore from backup
        let backup = dir.path().join("store.json.bak");
        assert!(backup.exists());
        let restored = std::fs::read_to_string(&backup).unwrap();
        std::fs::write(&path, &restored).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "important data");
    }

    // --- Corrupted persistence recovery ---

    #[test]
    fn load_events_binary_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("binary.jsonl");

        // Mix valid JSONL with binary data
        let valid = serde_json::to_string(&Event::SessionCancelled).unwrap();
        let mut content = valid.as_bytes().to_vec();
        content.push(b'\n');
        content.extend_from_slice(&[0x00, 0xFF, 0xFE, 0x80, 0x81]);
        content.push(b'\n');
        content.extend_from_slice(valid.as_bytes());
        content.push(b'\n');
        std::fs::write(&path, &content).unwrap();

        let events = load_events(&path).unwrap();
        // Should recover at least the first valid line
        assert!(!events.is_empty());
        assert!(matches!(&events[0], Event::SessionCancelled));
    }

    #[test]
    fn write_with_backup_corrupted_main_valid_backup_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.json");

        // Create original and backup
        std::fs::write(&path, "v1").unwrap();
        write_with_backup(&path, b"v2").unwrap();

        // Corrupt the main file
        std::fs::write(&path, [0xFF, 0xFE, 0x00]).unwrap();

        // Verify backup is still valid for recovery
        let backup = dir.path().join("data.json.bak");
        let backup_data = std::fs::read_to_string(&backup).unwrap();
        assert_eq!(backup_data, "v1");

        // Restore from backup
        std::fs::write(&path, &backup_data).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v1");
    }

    #[test]
    fn load_events_interleaved_valid_and_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("interleaved.jsonl");

        let line_started =
            serde_json::to_string(&Event::TaskStarted { task_id: TaskId(1) }).unwrap();
        let line_completed = serde_json::to_string(&Event::TaskCompleted {
            task_id: TaskId(2),
            summary: "done".into(),
        })
        .unwrap();
        let line_cancelled = serde_json::to_string(&Event::SessionCancelled).unwrap();

        let content = format!(
            "{line_started}\n\
             {{invalid json}}\n\
             {line_completed}\n\
             not even close to json\n\
             {line_cancelled}\n\
             {{\"type\": \"NonExistentVariant\"}}\n"
        );
        std::fs::write(&path, content).unwrap();

        let loaded = load_events(&path).unwrap();
        assert_eq!(loaded.len(), 3);
    }

    // --- Unicode in events ---

    #[test]
    fn event_unicode_roundtrip() {
        let events = vec![
            Event::AssistantMessage {
                content: "日本語テスト 🦀 中文消息 مرحبا".into(),
            },
            Event::TaskCompleted {
                task_id: TaskId(0),
                summary: "完了 ✅ terminé".into(),
            },
            Event::TaskFailed {
                task_id: TaskId(1),
                reason: "Ошибка: файл не найден".into(),
            },
            Event::ToolCall {
                name: "Read".into(),
                input: json!({"file_path": "/tmp/données/résumé.txt"}),
            },
        ];

        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let roundtripped: Event = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&roundtripped).unwrap();
            assert_eq!(json, json2, "Unicode roundtrip failed for {event:?}");
        }
    }

    // --- Concurrent logger writes ---

    #[test]
    fn logger_concurrent_writes() {
        let dir = tempfile::tempdir().unwrap();
        let session_id = SessionId("concurrent-test".into());
        let log_path = dir.path().join(format!("{session_id}.jsonl"));

        // Write events sequentially from multiple "logical" sources
        // (SessionLogger is not Send, so we test sequential rapid writes)
        let mut logger = SessionLogger::new(dir.path(), session_id).unwrap();
        for i in 0..50 {
            logger
                .log(&Event::TaskStarted { task_id: TaskId(i) })
                .unwrap();
            logger
                .log(&Event::TaskCompleted {
                    task_id: TaskId(i),
                    summary: format!("task {i} done"),
                })
                .unwrap();
        }
        drop(logger);

        // Verify all 100 events were written correctly
        let events = load_events(&log_path).unwrap();
        assert_eq!(events.len(), 100);
    }

    // --- write_with_backup with unicode content ---

    #[test]
    fn write_with_backup_unicode_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.json");

        let content = "日本語データ 🦀 données résumé";
        std::fs::write(&path, "original").unwrap();
        write_with_backup(&path, content.as_bytes()).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        let backup = dir.path().join("data.json.bak");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "original");
    }

    // --- SessionBanner and StepStarted tests ---

    #[test]
    fn session_banner_roundtrip() {
        let event = Event::SessionBanner {
            session_id: SessionId("20250206-143022".into()),
            datetime: "2025-02-06 14:30:22 PST".into(),
            target: TaskId(5),
            parent_branch: "main".into(),
            session_branch: "rloop-05".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let roundtripped: Event = serde_json::from_str(&json).unwrap();
        let json2 = serde_json::to_string(&roundtripped).unwrap();
        assert_eq!(json, json2);
    }

    #[test]
    fn session_banner_contains_fields() {
        let event = Event::SessionBanner {
            session_id: SessionId("20250206-143022".into()),
            datetime: "2025-02-06 14:30:22 PST".into(),
            target: TaskId(5),
            parent_branch: "main".into(),
            session_branch: "rloop-05".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("SessionBanner"));
        assert!(json.contains("20250206-143022"));
        assert!(json.contains("2025-02-06 14:30:22 PST"));
        assert!(json.contains("rloop-05"));
        assert!(json.contains("main"));
    }

    #[test]
    fn step_started_roundtrip() {
        let event = Event::StepStarted {
            task_id: TaskId(3),
            attempt: 1,
            model_id: "claude-sonnet-4-20250514".into(),
            cc_session_id: "abc-123".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let roundtripped: Event = serde_json::from_str(&json).unwrap();
        let json2 = serde_json::to_string(&roundtripped).unwrap();
        assert_eq!(json, json2);
    }

    #[test]
    fn step_started_contains_fields() {
        let event = Event::StepStarted {
            task_id: TaskId(3),
            attempt: 2,
            model_id: "claude-opus-4-20250514".into(),
            cc_session_id: "session-xyz-456".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("StepStarted"));
        assert!(json.contains("claude-opus-4-20250514"));
        assert!(json.contains("session-xyz-456"));
        assert!(json.contains(r#""attempt":2"#));
    }

    #[test]
    fn session_banner_logged_to_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let mut logger = SessionLogger::new(dir.path(), SessionId("test".into())).unwrap();
        let banner = Event::SessionBanner {
            session_id: SessionId("test".into()),
            datetime: "2025-02-06 14:30:22 PST".into(),
            target: TaskId(5),
            parent_branch: "main".into(),
            session_branch: "rloop-05".into(),
        };
        logger.log(&banner).unwrap();

        let path = dir.path().join("test.jsonl");
        let events = load_events(&path).unwrap();
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], Event::SessionBanner { session_id, .. } if session_id.0 == "test")
        );
    }

    #[test]
    fn step_started_logged_to_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let mut logger = SessionLogger::new(dir.path(), SessionId("test".into())).unwrap();
        let step = Event::StepStarted {
            task_id: TaskId(3),
            attempt: 1,
            model_id: "claude-sonnet-4-20250514".into(),
            cc_session_id: "abc-123".into(),
        };
        logger.log(&step).unwrap();

        let path = dir.path().join("test.jsonl");
        let events = load_events(&path).unwrap();
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], Event::StepStarted { task_id, attempt, .. } if *task_id == TaskId(3) && *attempt == 1)
        );
    }

    #[test]
    fn format_elapsed_seconds() {
        let d = std::time::Duration::from_secs(5);
        let result = format_elapsed(d);
        assert_eq!(result, "     +5s");
    }

    #[test]
    fn format_elapsed_minutes() {
        let d = std::time::Duration::from_secs(94); // 1m 34s
        let result = format_elapsed(d);
        assert_eq!(result, " +1m 34s"); // 8 chars with leading space
    }

    #[test]
    fn format_elapsed_zero() {
        let d = std::time::Duration::from_secs(0);
        let result = format_elapsed(d);
        assert_eq!(result, "     +0s");
    }

    #[test]
    fn format_elapsed_right_aligned() {
        // Verify all outputs are exactly 8 characters
        let cases = vec![
            std::time::Duration::from_secs(0),
            std::time::Duration::from_secs(9),
            std::time::Duration::from_secs(59),
            std::time::Duration::from_secs(60),
            std::time::Duration::from_secs(94),
            std::time::Duration::from_secs(600),
        ];
        for d in cases {
            let result = format_elapsed(d);
            assert_eq!(result.len(), 8, "elapsed time must be 8 chars: {:?}", result);
        }
    }
}
