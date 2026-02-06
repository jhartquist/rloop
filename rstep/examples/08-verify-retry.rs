/// ex-08: verify-after-complete with retry loop
///
/// Builds on ex-07. The `complete` tool handler runs `cargo test` internally
/// before accepting the result:
///
/// - If tests **pass** → store summary, set stop flag, return success.
/// - If tests **fail** → return the test output with `is_error: true`.
///   Claude sees the failure and retries.
///
/// Key design choices:
/// - **No Bash tool** — Claude cannot run `cargo test` itself. The only way
///   to verify is through `complete`. This forces the retry loop.
/// - **`two-bugs` template** — has two buggy functions. The prompt says "fix
///   the bug" (singular), biasing Claude toward fixing only one on first pass.
/// - **Attempt counter** — tracks how many times `complete` was called.
/// - **`output_format` fallback** — if Claude gives up without calling
///   `complete`, we still get structured JSON.
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use claude_agent_sdk_rs::{
    ClaudeAgentOptions, ClaudeClient, McpServerConfig, McpServers, McpToolResultContent, Message,
    ToolResult, create_sdk_mcp_server, tool,
};
use colored::Colorize;
use futures::StreamExt;
use serde_json::json;
use strum::IntoEnumIterator;

use rstep::{
    helpers::{self, Template},
    print::EventPrinter,
    tool::BuiltinTool,
};

#[tokio::main]
async fn main() -> Result<()> {
    let temp_dir = helpers::copy_template(Template::TwoBugs)?;
    let work_dir: PathBuf = temp_dir.path().to_path_buf();

    // Shared state
    let summary: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let stop: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    let attempts: Arc<AtomicU32> = Arc::new(AtomicU32::new(0));

    // -- Define the `complete` MCP tool with verification -----------------------

    let summary_clone = Arc::clone(&summary);
    let stop_clone = Arc::clone(&stop);
    let attempts_clone = Arc::clone(&attempts);
    let work_dir_clone = work_dir.clone();

    let complete_tool = tool!(
        "complete",
        "Call this tool when you have finished the task. Runs cargo test to verify. \
         If tests fail, you'll get the output back to fix the remaining issues.",
        json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Summary of what was accomplished"
                }
            },
            "required": ["summary"]
        }),
        move |args: serde_json::Value| {
            let summary_inner = Arc::clone(&summary_clone);
            let stop_inner = Arc::clone(&stop_clone);
            let attempts_inner = Arc::clone(&attempts_clone);
            let dir = work_dir_clone.clone();
            async move {
                let attempt = attempts_inner.fetch_add(1, Ordering::SeqCst) + 1;
                let text = args
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(no summary provided)")
                    .to_string();

                // Run cargo test in the working directory
                let output = std::process::Command::new("cargo")
                    .args(["test"])
                    .current_dir(&dir)
                    .output()
                    .map_err(|e| anyhow::anyhow!("Failed to run cargo test: {e}"))?;

                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                if output.status.success() {
                    // Tests passed — accept the completion
                    *summary_inner.lock().unwrap() = Some(text);
                    stop_inner.store(true, Ordering::SeqCst);

                    Ok(ToolResult {
                        content: vec![McpToolResultContent::Text {
                            text: format!("Tests passed (attempt {attempt}). Task complete."),
                        }],
                        is_error: false,
                    })
                } else {
                    // Tests failed — return output so Claude can fix and retry
                    let msg = format!(
                        "Tests FAILED (attempt {attempt}). Fix the remaining issues and call complete again.\n\n\
                         --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
                    );

                    Ok(ToolResult {
                        content: vec![McpToolResultContent::Text { text: msg }],
                        is_error: true,
                    })
                }
            }
        }
    );

    // -- Create MCP server and register it --------------------------------------

    let server = create_sdk_mcp_server("rstep", "0.1.0", vec![complete_tool]);

    let mut servers = HashMap::new();
    servers.insert("rstep".to_string(), McpServerConfig::Sdk(server));

    // -- Structured output schema (fallback only) --------------------------------

    let output_format = json!({
        "type": "json_schema",
        "schema": {
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Summary of what happened"
                },
                "learnings": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "What was learned during the attempt"
                }
            },
            "required": ["summary", "learnings"]
        }
    });

    // -- Build allowed / disallowed tool lists ----------------------------------
    //
    // No Bash tool — Claude can't run cargo test itself.
    // The only way to verify is through the `complete` tool.

    let allowed_builtins: HashSet<BuiltinTool> =
        [BuiltinTool::Read, BuiltinTool::Edit].into_iter().collect();

    let disallowed_builtins: HashSet<BuiltinTool> = BuiltinTool::iter()
        .collect::<HashSet<_>>()
        .difference(&allowed_builtins)
        .copied()
        .collect();

    let mut allowed_tools: Vec<String> = allowed_builtins.iter().map(|t| t.to_string()).collect();
    allowed_tools.push("mcp__rstep__complete".to_string());

    let disallowed_tools: Vec<String> = disallowed_builtins.iter().map(|t| t.to_string()).collect();

    // -- Build client options ---------------------------------------------------

    let options = ClaudeAgentOptions::builder()
        .model("Haiku".to_string())
        .cwd(temp_dir.path())
        .setting_sources(vec![])
        .allowed_tools(allowed_tools)
        .disallowed_tools(disallowed_tools)
        .mcp_servers(McpServers::Dict(servers))
        .output_format(output_format)
        .build();

    let mut client = ClaudeClient::new(options);

    // -- Connect, query, stream ------------------------------------------------

    client.connect().await?;

    let prompt = "The add function in src/lib.rs has a bug. Read the file, fix it, \
                  then call the complete tool with a summary.";

    println!("{}  {}\n", "PROMPT".blue().bold(), prompt);

    client.query(prompt).await?;

    let mut printer = EventPrinter::new(temp_dir.path());
    let mut messages: Vec<Message> = Vec::new();
    let mut interrupted = false;
    {
        let mut stream = client.receive_response();
        while let Some(result) = stream.next().await {
            let msg = result?;
            printer.print(&msg);
            messages.push(msg);
            if !interrupted && stop.load(Ordering::SeqCst) {
                client.interrupt().await.ok();
                interrupted = true;
            }
        }
    }

    client.disconnect().await?;

    // -- Print outcome ---------------------------------------------------------

    let total_attempts = attempts.load(Ordering::SeqCst);

    if let Some(text) = summary.lock().unwrap().as_ref() {
        println!(
            "\n{}  {} (after {} attempt{})",
            "SUCCESS".green().bold(),
            text,
            total_attempts,
            if total_attempts == 1 { "" } else { "s" }
        );
    } else {
        println!(
            "\n{}  complete tool was not called or tests never passed ({} attempt{})",
            "FAILED".red().bold(),
            total_attempts,
            if total_attempts == 1 { "" } else { "s" }
        );

        if let Some(Message::Result(res)) = messages
            .iter()
            .rev()
            .find(|m| matches!(m, Message::Result(_)))
            && let Some(output) = &res.structured_output
        {
            println!(
                "{}  {}",
                "FALLBACK".yellow().bold(),
                serde_json::to_string_pretty(output)?
            );
        }
    }

    if let Some(Message::Result(res)) = messages
        .iter()
        .rev()
        .find(|m| matches!(m, Message::Result(_)))
    {
        let cost = res.total_cost_usd.unwrap_or(0.0);
        let duration = res.duration_ms as f64 / 1000.0;
        println!("{}  ${cost:.6} · {duration:.2}s", "  COST".dimmed());
    }

    Ok(())
}
