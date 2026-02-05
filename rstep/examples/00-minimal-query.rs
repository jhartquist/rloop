//! Example 00: Minimal Query with Verbose Output
//!
//! The absolute minimum to call Claude and see everything that comes back.
//!
//! What this does:
//! 1. Creates a temp directory (clean slate, no CLAUDE.md)
//! 2. Calls Claude with "What is 2+2?"
//! 3. Prints EVERY message as raw JSON (pipe to `jq` for readability)
//! 4. Accumulates and prints token counts
//! 5. Cleans up temp directory on exit
//!
//! Run with:
//!   cargo run --example 00-minimal-query 2>&1 | jq .
//!
//! Or for raw output:
//!   cargo run --example 00-minimal-query

use anyhow::Result;
use claude_agent_sdk_rs::{ClaudeAgentOptions, ClaudeClient, Message, PermissionMode};
use futures::StreamExt;
use tempfile::tempdir;

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Create temp directory - this gives us a clean slate with no ambient context
    let temp = tempdir()?;
    eprintln!("Working directory: {:?}", temp.path());

    // 2. Configure with maximum control:
    //    - Use Haiku for cost-effective testing
    //    - Empty allowed_tools = no tools enabled
    //    - BypassPermissions so we don't get permission prompts
    //    - Working directory set to our temp dir
    let options = ClaudeAgentOptions::builder()
        .model("claude-haiku-4-5-20251001")
        .allowed_tools(Vec::<String>::new()) // No tools
        .permission_mode(PermissionMode::BypassPermissions)
        .cwd(temp.path())
        .build();

    // 3. Create client and connect
    let mut client = ClaudeClient::new(options);
    client.connect().await?;

    // 4. Send our query
    client.query("What is 2+2?").await?;

    // 5. Receive and print all messages, tracking tokens
    let mut total_input_tokens: u64 = 0;
    let mut total_output_tokens: u64 = 0;

    {
        let mut stream = client.receive_response();
        while let Some(result) = stream.next().await {
            match result {
                Ok(message) => {
                    // Print the raw message as JSON for verbose output
                    let json = serde_json::to_string_pretty(&message)?;
                    println!("{}", json);

                    // Track tokens based on message type
                    // Usage is serde_json::Value, so we access fields via .get()
                    match &message {
                        Message::Assistant(msg) => {
                            if let Some(usage) = &msg.message.usage {
                                if let Some(input) = usage.get("input_tokens").and_then(|v| v.as_u64()) {
                                    total_input_tokens += input;
                                }
                                if let Some(output) = usage.get("output_tokens").and_then(|v| v.as_u64()) {
                                    total_output_tokens += output;
                                }
                            }
                        }
                        Message::Result(result) => {
                            if let Some(usage) = &result.usage {
                                if let Some(input) = usage.get("input_tokens").and_then(|v| v.as_u64()) {
                                    total_input_tokens += input;
                                }
                                if let Some(output) = usage.get("output_tokens").and_then(|v| v.as_u64()) {
                                    total_output_tokens += output;
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Err(e) => {
                    eprintln!("Error receiving message: {}", e);
                }
            }
        }
    }

    // 6. Disconnect
    client.disconnect().await?;

    // 7. Print token summary
    eprintln!("---");
    eprintln!("Token Summary:");
    eprintln!("  Input tokens:  {}", total_input_tokens);
    eprintln!("  Output tokens: {}", total_output_tokens);
    eprintln!("  Total tokens:  {}", total_input_tokens + total_output_tokens);

    // temp dir is automatically cleaned up when `temp` goes out of scope

    Ok(())
}
