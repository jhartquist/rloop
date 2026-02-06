use std::{collections::HashMap, path::PathBuf};

use claude_agent_sdk_rs::Message;
use colored::Colorize;

pub struct EventPrinter {
    project_dir: PathBuf,
    pending_tools: HashMap<String, (String, serde_json::Value)>,
}

impl EventPrinter {
    pub fn new(project_dir: impl Into<PathBuf>) -> Self {
        let project_dir = project_dir.into();
        let project_dir = project_dir.canonicalize().unwrap_or(project_dir);
        Self {
            project_dir,
            pending_tools: HashMap::new(),
        }
    }

    pub fn print(&mut self, msg: &Message) {
        match msg {
            Message::System(sys) => {
                let label = "SYSTEM".magenta().bold();
                let session = sys.session_id.as_deref().unwrap_or("unknown");
                let model = sys.model.as_deref().unwrap_or("unknown");
                let tools = sys.tools.as_ref().map(|t| t.join(", ")).unwrap_or_default();
                println!(
                    "{label}  {}",
                    format!("session {session} · {model} · tools: {tools}").dimmed()
                );
            }

            Message::Assistant(asst) => {
                for block in &asst.message.content {
                    match block {
                        claude_agent_sdk_rs::ContentBlock::Text(t) => {
                            let label = "  ASST".cyan().bold();
                            let pad = "      ";
                            for (i, line) in t.text.lines().enumerate() {
                                if i == 0 {
                                    println!("{label}  {line}");
                                } else {
                                    println!("{pad}  {line}");
                                }
                            }
                        }
                        claude_agent_sdk_rs::ContentBlock::ToolUse(tu) => {
                            self.pending_tools
                                .insert(tu.id.clone(), (tu.name.clone(), tu.input.clone()));
                            self.print_tool_call(&tu.name, &tu.input);
                        }
                        _ => {}
                    }
                }
            }

            Message::User(user) => {
                // SDK bug: typed fields (content, parent_tool_use_id) are None.
                // Tool results land in extra.tool_use_result and extra.message.content.
                let tool_use_id = user
                    .extra
                    .pointer("/message/content/0/tool_use_id")
                    .and_then(|v| v.as_str());

                let tool_info = tool_use_id.and_then(|id| self.pending_tools.remove(id));

                let tur = user.extra.get("tool_use_result");
                let content_text = user
                    .extra
                    .pointer("/message/content/0/content")
                    .and_then(|v| v.as_str());

                if tur.is_some() || content_text.is_some() {
                    self.print_tool_result(tool_info.as_ref(), tur, content_text);
                    return;
                }

                // Debug: dump unknown User message structures
                if std::env::var("RSTEP_DEBUG").is_ok() {
                    eprintln!(
                        "DEBUG user.extra: {}",
                        serde_json::to_string_pretty(&user.extra).unwrap_or_default()
                    );
                }
            }

            Message::Result(res) => {
                let label = "RESULT".green().bold();
                let duration = res.duration_ms as f64 / 1000.0;
                let cost = res.total_cost_usd.unwrap_or(0.0);
                println!(
                    "{label}  {}",
                    format!("{} turns · ${cost:.6} · {duration:.2}s", res.num_turns).dimmed()
                );
            }

            Message::StreamEvent(_) | Message::ControlCancelRequest(_) => return,
        }
        println!();
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
                println!("{label}  {} {}", "Read".dimmed(), self.rel(path).dimmed());
            }
            "Edit" => {
                let path = input
                    .get("file_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                println!("{label}  {} {}", "Edit".dimmed(), self.rel(path).dimmed());
                if let Some(old) = input.get("old_string").and_then(|v| v.as_str()) {
                    for line in old.lines() {
                        println!("{pad}  {} {}", "-".red(), line.dimmed());
                    }
                }
                if let Some(new) = input.get("new_string").and_then(|v| v.as_str()) {
                    for line in new.lines() {
                        println!("{pad}  {} {}", "+".green(), line.dimmed());
                    }
                }
            }
            "Write" => {
                let path = input
                    .get("file_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                println!("{label}  {} {}", "Write".dimmed(), self.rel(path).dimmed());
                if let Some(content) = input.get("content").and_then(|v| v.as_str()) {
                    for line in content.lines() {
                        println!("{pad}  {}", line.dimmed());
                    }
                }
            }
            "Bash" => {
                let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("?");
                println!(
                    "{label}  {} {}",
                    "Bash".dimmed(),
                    self.shorten(cmd).dimmed()
                );
            }
            other => {
                let param = self.extract_key_param(input);
                if param.is_empty() {
                    println!("{label}  {}", other.dimmed());
                } else {
                    println!("{label}  {} {}", other.dimmed(), param.dimmed());
                }
            }
        }
    }

    fn print_tool_result(
        &self,
        tool_info: Option<&(String, serde_json::Value)>,
        tur: Option<&serde_json::Value>,
        content_text: Option<&str>,
    ) {
        let label = "   RES".yellow().bold();
        let name = tool_info.map(|(n, _)| n.as_str()).unwrap_or("?");

        // Read result: { "file": { "filePath": "...", "content": "...", "numLines": N } }
        if let Some(file) = tur.and_then(|t| t.get("file")) {
            let path = file.get("filePath").and_then(|v| v.as_str()).unwrap_or("?");
            let num_lines = file.get("numLines").and_then(|v| v.as_u64()).unwrap_or(0);
            let content = file.get("content").and_then(|v| v.as_str()).unwrap_or("");

            println!(
                "{label}  {}",
                format!("{} ({num_lines} lines)", self.rel(path)).dimmed()
            );
            for line in content.lines() {
                println!("        {}", line.dimmed());
            }
            return;
        }

        // Write/Edit: details already shown in CALL, just confirm
        if matches!(name, "Write" | "Edit") {
            println!("{label}  {}", format!("{name} ok").dimmed());
            return;
        }

        // Try to extract text from tool_use_result (various possible shapes)
        if let Some(text) = tur
            .and_then(|t| {
                t.as_str()
                    .or_else(|| t.get("text").and_then(|v| v.as_str()))
                    .or_else(|| t.get("content").and_then(|v| v.as_str()))
                    .or_else(|| t.get("stdout").and_then(|v| v.as_str()))
                    .or_else(|| t.get("output").and_then(|v| v.as_str()))
            })
            .or(content_text)
        {
            println!("{label}  {}", name.dimmed());
            for line in text.lines() {
                println!("        {}", self.shorten(line).dimmed());
            }
            return;
        }

        println!("{label}  {}", format!("{name} ok").dimmed());

        // Debug: dump unknown result structures
        if std::env::var("RSTEP_DEBUG").is_ok() {
            if let Some(t) = tur {
                eprintln!(
                    "DEBUG tool_use_result: {}",
                    serde_json::to_string_pretty(t).unwrap_or_default()
                );
            }
        }
    }

    /// Strip project_dir prefix from a path string.
    fn rel(&self, path: &str) -> String {
        let prefix = self.project_dir.to_string_lossy();
        path.strip_prefix(prefix.as_ref())
            .map(|p| p.strip_prefix('/').unwrap_or(p))
            .unwrap_or(path)
            .to_string()
    }

    /// Replace project_dir prefix anywhere in a string (for commands, output).
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
