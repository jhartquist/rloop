use std::collections::HashSet;

use anyhow::Result;
use claude_agent_sdk_rs::{ClaudeAgentOptions, ClaudeClient, Message};
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

    // Define the structured output schema.
    // This forces Claude to produce JSON matching this shape as its final output,
    // even when it can't complete the task (no Edit tool = guaranteed failure).
    let output_format = json!({
        "type": "json_schema",
        "schema": {
            "type": "object",
            "properties": {
                "outcome": {
                    "type": "string",
                    "enum": ["success", "failed", "blocked"],
                    "description": "Whether the task succeeded, failed, or is blocked"
                },
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
            "required": ["outcome", "summary", "learnings"]
        }
    });

    // Only allow Read — Claude can see the bug but can't fix it.
    let allowed: HashSet<BuiltinTool> = [BuiltinTool::Read].into_iter().collect();
    let disallowed: Vec<String> = BuiltinTool::iter()
        .filter(|t| !allowed.contains(t))
        .map(|t| t.to_string())
        .collect();

    let options = ClaudeAgentOptions::builder()
        .model("Haiku".to_string())
        .cwd(temp_dir.path())
        .setting_sources(vec![])
        .output_format(output_format)
        .allowed_tools(allowed.iter().map(|t| t.to_string()).collect::<Vec<_>>())
        .disallowed_tools(disallowed)
        .build();

    let mut client = ClaudeClient::new(options);
    client.connect().await?;

    let prompt = "Read src/lib.rs, find the bug, and fix it. If you cannot fix it, explain why.";

    println!("{}  {}\n", "PROMPT".blue().bold(), prompt);

    client.query(prompt).await?;

    // Stream and print all messages.
    let mut printer = EventPrinter::new(temp_dir.path());
    let mut messages = Vec::new();
    {
        let mut stream = client.receive_response();
        while let Some(result) = stream.next().await {
            let msg = result?;
            printer.print(&msg);
            messages.push(msg);
        }
    }

    client.disconnect().await?;

    // Extract structured output from the ResultMessage.
    if let Some(Message::Result(res)) = messages.last() {
        if let Some(output) = &res.structured_output {
            println!("\n{}  Structured output:", "OUTPUT".magenta().bold());
            println!("{}", serde_json::to_string_pretty(output)?);
        }

        let cost = res.total_cost_usd.unwrap_or(0.0);
        let duration = res.duration_ms as f64 / 1000.0;
        println!("\n{}  ${cost:.6} · {duration:.2}s", "  COST".dimmed());
    }

    Ok(())
}
