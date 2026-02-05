//! Example 00: Minimal Query with Verbose Output
//!
//! The absolute minimum to call Claude and see everything that comes back.
//!
//! What this does:
//! 1. Creates a temp directory (clean slate, no CLAUDE.md)
//! 2. Calls Claude with "What is 2+2?"
//! 3. Prints each message with dbg!()
//! 4. Prints a summary of message types, tokens, and cost
//! 5. Calculates cost from tokens and compares to API-reported cost
//! 6. Cleans up temp directory on exit
//!
//! Run with:
//!   cargo run --example 00-minimal-query

use anyhow::Result;
use claude_agent_sdk_rs::{ClaudeAgentOptions, ClaudeClient, Message, PermissionMode};
use futures::StreamExt;
use tempfile::tempdir;

/// Model pricing in USD per million tokens (MTok)
/// From: https://docs.anthropic.com/en/docs/about-claude/pricing
struct ModelPricing {
    input: f64,        // Base input tokens
    output: f64,       // Output tokens
    cache_write: f64,  // 5m cache writes (1.25x input)
    cache_read: f64,   // Cache hits (0.1x input)
}

impl ModelPricing {
    /// Calculate cost from token counts
    fn calculate_cost(&self, input: u64, output: u64, cache_read: u64, cache_create: u64) -> f64 {
        let mtok = 1_000_000.0;
        (input as f64 * self.input / mtok)
            + (output as f64 * self.output / mtok)
            + (cache_read as f64 * self.cache_read / mtok)
            + (cache_create as f64 * self.cache_write / mtok)
    }
}

fn get_model_pricing(model: &str) -> Option<ModelPricing> {
    match model {
        s if s.contains("haiku-4-5") => Some(ModelPricing {
            input: 1.0,
            output: 5.0,
            cache_write: 1.25,
            cache_read: 0.10,
        }),
        s if s.contains("haiku-3-5") => Some(ModelPricing {
            input: 0.80,
            output: 4.0,
            cache_write: 1.0,
            cache_read: 0.08,
        }),
        s if s.contains("sonnet-4-5") || s.contains("sonnet-4") || s.contains("sonnet-3-7") => {
            Some(ModelPricing {
                input: 3.0,
                output: 15.0,
                cache_write: 3.75,
                cache_read: 0.30,
            })
        }
        s if s.contains("opus-4-5") => Some(ModelPricing {
            input: 5.0,
            output: 25.0,
            cache_write: 6.25,
            cache_read: 0.50,
        }),
        s if s.contains("opus-4") || s.contains("opus-3") => Some(ModelPricing {
            input: 15.0,
            output: 75.0,
            cache_write: 18.75,
            cache_read: 1.50,
        }),
        _ => None,
    }
}

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
        .permission_mode(PermissionMode::BypassPermissions)
        .cwd(temp.path())
        .setting_sources(vec![])
        .allowed_tools(Vec::<String>::new()) // No tools
        .disallowed_tools(
            [
                "Task",
                "TaskOutput",
                "Bash",
                "Glob",
                "Grep",
                "ExitPlanMode",
                "Read",
                "Edit",
                "Write",
                "NotebookEdit",
                "WebFetch",
                "TodoWrite",
                "WebSearch",
                "TaskStop",
                "AskUserQuestion",
                "Skill",
                "EnterPlanMode",
                "ToolSearch",
            ]
            .map(String::from)
            .to_vec(),
        )
        .build();

    // 3. Create client and connect
    let mut client = ClaudeClient::new(options);
    client.connect().await?;

    // 4. Send our query
    client.query("What is 2+2?").await?;

    // 5. Receive and print all messages, tracking counts
    let mut system_count = 0u32;
    let mut assistant_count = 0u32;
    let mut result_count = 0u32;
    let mut other_count = 0u32;

    // We'll capture the final result message and model for the summary
    let mut final_result: Option<claude_agent_sdk_rs::ResultMessage> = None;
    let mut model_name: Option<String> = None;

    {
        let mut stream = client.receive_response();
        while let Some(result) = stream.next().await {
            match result {
                Ok(message) => {
                    dbg!(&message);

                    match &message {
                        Message::System(s) => {
                            system_count += 1;
                            model_name = s.model.clone();
                        }
                        Message::Assistant(_) => assistant_count += 1,
                        Message::Result(r) => {
                            result_count += 1;
                            final_result = Some(r.clone());
                        }
                        _ => other_count += 1,
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

    // 7. Print summary
    eprintln!("\n========== Session Summary ==========");
    eprintln!("Messages received:");
    eprintln!("  System:    {}", system_count);
    eprintln!("  Assistant: {}", assistant_count);
    eprintln!("  Result:    {}", result_count);
    if other_count > 0 {
        eprintln!("  Other:     {}", other_count);
    }

    if let Some(result) = final_result {
        let model = model_name.as_deref().unwrap_or("unknown");
        eprintln!("\nModel: {}", model);
        eprintln!("Outcome: {}", result.subtype);
        eprintln!("Duration: {}ms (API: {}ms)", result.duration_ms, result.duration_api_ms);
        eprintln!("Turns: {}", result.num_turns);

        if let Some(usage) = &result.usage {
            let input = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let output = usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let cache_read = usage.get("cache_read_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let cache_create = usage.get("cache_creation_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);

            eprintln!("\nTokens:");
            eprintln!("  Input:        {}", input);
            eprintln!("  Output:       {}", output);
            if cache_read > 0 {
                eprintln!("  Cache read:   {}", cache_read);
            }
            if cache_create > 0 {
                eprintln!("  Cache create: {}", cache_create);
            }
            eprintln!("  Total:        {}", input + output);

            // Calculate cost from tokens and compare to API-reported cost
            eprintln!("\nCost:");
            if let Some(pricing) = get_model_pricing(model) {
                let calculated = pricing.calculate_cost(input, output, cache_read, cache_create);
                eprintln!("  Calculated:   ${:.6}", calculated);
                eprintln!("    Input:      {} × ${}/MTok = ${:.6}", input, pricing.input, input as f64 * pricing.input / 1_000_000.0);
                eprintln!("    Output:     {} × ${}/MTok = ${:.6}", output, pricing.output, output as f64 * pricing.output / 1_000_000.0);
                if cache_read > 0 {
                    eprintln!("    Cache read: {} × ${}/MTok = ${:.6}", cache_read, pricing.cache_read, cache_read as f64 * pricing.cache_read / 1_000_000.0);
                }
                if cache_create > 0 {
                    eprintln!("    Cache write: {} × ${}/MTok = ${:.6}", cache_create, pricing.cache_write, cache_create as f64 * pricing.cache_write / 1_000_000.0);
                }

                if let Some(api_cost) = result.total_cost_usd {
                    eprintln!("  API reported: ${:.6}", api_cost);
                    let diff = (calculated - api_cost).abs();
                    if diff < 0.000001 {
                        eprintln!("  ✓ Costs match!");
                    } else {
                        eprintln!("  Δ Difference:  ${:.6}", diff);
                    }
                }
            } else if let Some(api_cost) = result.total_cost_usd {
                eprintln!("  API reported: ${:.6}", api_cost);
                eprintln!("  (Unknown model pricing for calculation)");
            }
        }

        if let Some(answer) = &result.result {
            eprintln!("\nAnswer: {}", answer);
        }
    }
    eprintln!("======================================");

    // temp dir is automatically cleaned up when `temp` goes out of scope

    Ok(())
}
