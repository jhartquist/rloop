/// ex-07: structured output as failure fallback
///
/// Builds on ex-06 (MCP complete tool). Combines two completion mechanisms:
///
/// 1. **`complete` MCP tool** — the success path. Claude calls this when done.
/// 2. **`output_format`** — the failure safety net. If Claude never calls `complete`
///    (e.g., it gives up or runs out of turns), structured output guarantees we still
///    get parseable JSON with a summary and learnings.
///
/// The schema has no `outcome` field — if we reach structured output, it's always a
/// failure (success goes through the tool). This simplifies parsing.
///
/// Claude gets Read, Edit, Bash, and mcp__rstep__complete. With full tools it should
/// succeed via `complete`. The structured output fallback is the safety net.
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
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
    let temp_dir = helpers::copy_template(Template::BugAdder)?;

    // Shared state: the complete tool writes here, main reads after session ends.
    let summary: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let stop: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

    // -- Define the `complete` MCP tool -----------------------------------------

    let summary_clone = Arc::clone(&summary);
    let stop_clone = Arc::clone(&stop);

    let complete_tool = tool!(
        "complete",
        "Call this tool when you have finished the task. Provide a summary of what you did.",
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
            async move {
                let text = args
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(no summary provided)")
                    .to_string();

                *summary_inner.lock().unwrap() = Some(text);
                stop_inner.store(true, Ordering::SeqCst);

                Ok(ToolResult {
                    content: vec![McpToolResultContent::Text {
                        text: "Task marked complete.".into(),
                    }],
                    is_error: false,
                })
            }
        }
    );

    // -- Create MCP server and register it --------------------------------------

    let server = create_sdk_mcp_server("rstep", "0.1.0", vec![complete_tool]);

    let mut servers = HashMap::new();
    servers.insert("rstep".to_string(), McpServerConfig::Sdk(server));

    // -- Structured output schema (fallback only) --------------------------------
    //
    // No `outcome` field — if we hit this path, it's always a failure.
    // Success goes through the `complete` tool.
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

    let allowed_builtins: HashSet<BuiltinTool> =
        [BuiltinTool::Read, BuiltinTool::Edit, BuiltinTool::Bash]
            .into_iter()
            .collect();

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

    let prompt = "Read src/lib.rs, find and fix the bug, run cargo test to verify, \
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

    if let Some(text) = summary.lock().unwrap().as_ref() {
        println!("\n{}  {}", "SUCCESS".green().bold(), text);
    } else {
        // Fell through to structured output — always a failure.
        println!("\n{}  complete tool was not called", "FAILED".red().bold());

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
