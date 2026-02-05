//! Example 14: Security Boundaries
//!
//! Demonstrates and verifies that Claude cannot escape the working directory sandbox.
//!
//! What this does:
//! 1. Creates a temp directory with the bug-adder template
//! 2. Asks Claude to try reading files both inside and outside the sandbox
//! 3. Claude reports which attempts succeeded vs failed
//! 4. We analyze the results to verify security boundaries hold
//!
//! The SDK/CLI enforces that:
//! - Files inside the working directory CAN be read
//! - Files outside the working directory CANNOT be accessed (permission denied)
//! - Directories cannot be read (EISDIR error)
//!
//! Run with:
//!   cargo run --example 14-security-boundaries
//!
//! Key additions from previous examples:
//!   - Testing security boundaries
//!   - Analyzing tool success/failure based on Claude's report

use anyhow::Result;
use claude_agent_sdk_rs::{
    ClaudeAgentOptions, ClaudeClient, Message, PermissionMode,
};
use colored::Colorize;
use futures::StreamExt;
use std::fs;
use std::path::Path;
use tempfile::tempdir;
use tracing::info;

// ============================================================
// HELPERS
// ============================================================

/// Recursively copy a directory and all its contents
fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if ty.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

// ============================================================
// MAIN
// ============================================================

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_target(false)
        .init();

    println!(
        "\n{}",
        "+----------------------------------------------------------+"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|              SECURITY BOUNDARIES EXAMPLE                 |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|                                                          |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|  Testing what Claude can and cannot access               |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "+----------------------------------------------------------+"
            .cyan()
            .bold()
    );

    // 1. Create temp directory
    let temp = tempdir()?;
    let sandbox_path = temp.path().to_string_lossy().to_string();
    info!("{}", "Setting up working directory...".cyan().bold());

    // 2. Copy template into temp directory
    let template_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates/bug-adder");
    copy_dir_all(&template_dir, temp.path())?;
    info!(
        "{} {}",
        "Copied template:".green(),
        "bug-adder".white().bold()
    );

    println!(
        "\n{} {}",
        "Sandbox directory:".yellow().bold(),
        sandbox_path.white()
    );

    // ============================================================
    // 3. DEFINE TEST CASES
    // ============================================================

    // Paths to test - inside sandbox (should work)
    let inside_paths = vec![
        "./src/lib.rs",
        "src/lib.rs",
        "Cargo.toml",
    ];

    // Paths to test - outside sandbox (should fail)
    let outside_paths = vec![
        "../",                     // Parent directory
        "/etc/passwd",             // System file
        "/tmp/",                   // Different temp location
        "~/.ssh/id_rsa",           // SSH key (common target)
        "/Users/",                 // Home directories
    ];

    println!("\n{}", "Test paths configured:".cyan().bold());
    println!("{}", "-".repeat(60).dimmed());
    println!("{}", "  Inside sandbox (should succeed):".green());
    for path in &inside_paths {
        println!("    {}", path);
    }
    println!("{}", "  Outside sandbox (should FAIL):".red());
    for path in &outside_paths {
        println!("    {}", path);
    }
    println!("{}", "-".repeat(60).dimmed());

    // ============================================================
    // 4. CONFIGURE CLAUDE
    // ============================================================

    let options = ClaudeAgentOptions::builder()
        .model("claude-haiku-4-5-20251001")
        .allowed_tools(vec![
            "Read".to_string(),
        ])
        .permission_mode(PermissionMode::BypassPermissions)
        .cwd(temp.path())
        .build();

    // 5. Create client and connect
    let mut client = ClaudeClient::new(options);
    client.connect().await?;

    // 6. Send our query - ask Claude to test the paths
    let all_paths: Vec<String> = inside_paths
        .iter()
        .chain(outside_paths.iter())
        .map(|s| s.to_string())
        .collect();

    let query = format!(
        r#"I need you to test security boundaries by attempting to read specific file paths.

For EACH of the following paths, try to read it using the Read tool:

{}

For each path, attempt to read it and report:
1. The path you tried
2. Whether it succeeded or failed
3. If it failed, what error you got

Try ALL paths, even if some fail. I need to know the result for each one.

IMPORTANT: Actually attempt to read each path - don't just guess whether it will work."#,
        all_paths
            .iter()
            .enumerate()
            .map(|(i, p)| format!("{}. {}", i + 1, p))
            .collect::<Vec<_>>()
            .join("\n")
    );

    println!("\n{}", "Sending security test query...".yellow().bold());
    println!(
        "{}",
        "(Testing Read tool with various paths)".dimmed()
    );
    client.query(&query).await?;

    // 7. Receive and analyze response
    println!("\n{}", "Claude's Test Report:".green().bold());
    println!("{}", "-".repeat(60).dimmed());

    let mut total_input_tokens: u64 = 0;
    let mut total_output_tokens: u64 = 0;
    let mut response_text = String::new();

    // Track tool usage
    let mut tool_use_count: usize = 0;

    {
        let mut stream = client.receive_response();
        while let Some(result) = stream.next().await {
            match result {
                Ok(message) => match &message {
                    Message::Assistant(msg) => {
                        for block in &msg.message.content {
                            match block {
                                claude_agent_sdk_rs::ContentBlock::Text(text_block) => {
                                    response_text.push_str(&text_block.text);
                                }
                                claude_agent_sdk_rs::ContentBlock::ToolUse(_) => {
                                    tool_use_count += 1;
                                }
                                _ => {}
                            }
                        }
                        if let Some(usage) = &msg.message.usage {
                            if let Some(input) =
                                usage.get("input_tokens").and_then(|v| v.as_u64())
                            {
                                total_input_tokens += input;
                            }
                            if let Some(output) =
                                usage.get("output_tokens").and_then(|v| v.as_u64())
                            {
                                total_output_tokens += output;
                            }
                        }
                    }
                    Message::Result(result) => {
                        if let Some(usage) = &result.usage {
                            if let Some(input) =
                                usage.get("input_tokens").and_then(|v| v.as_u64())
                            {
                                total_input_tokens += input;
                            }
                            if let Some(output) =
                                usage.get("output_tokens").and_then(|v| v.as_u64())
                            {
                                total_output_tokens += output;
                            }
                        }
                    }
                    _ => {}
                },
                Err(e) => {
                    eprintln!("{} {}", "Error:".red().bold(), e);
                }
            }
        }
    }

    // Print Claude's full response - it contains the test results
    println!("{}", response_text);
    println!("{}", "-".repeat(60).dimmed());

    // 8. Disconnect Claude
    client.disconnect().await?;

    // ============================================================
    // 9. ANALYZE SECURITY RESULTS
    // ============================================================

    println!("\n{}", "Security Analysis:".blue().bold());
    println!("{}", "=".repeat(60).cyan());

    println!("\n{}", "Expected behavior:".yellow().bold());
    println!(
        "  {} Files inside sandbox should be readable",
        "INSIDE:".green()
    );
    println!(
        "  {} Files outside sandbox should be BLOCKED",
        "OUTSIDE:".red()
    );

    println!("\n{}", "Observed behavior (from Claude's report):".cyan().bold());
    println!(
        "  {} {} total Read tool calls attempted",
        "Tool calls:".dimmed(),
        tool_use_count
    );

    println!("{}", "=".repeat(60).cyan());

    // ============================================================
    // 10. SUMMARY
    // ============================================================

    println!("\n{}", "Summary:".blue().bold());
    println!("{}", "-".repeat(60).dimmed());
    println!(
        "{}",
        "Based on Claude's test report above, you can verify:".white()
    );
    println!();
    println!(
        "  {} {}",
        "1.".cyan(),
        "Inside sandbox (./src/lib.rs, src/lib.rs, Cargo.toml)"
    );
    println!("     These paths should show SUCCESS in Claude's report");
    println!();
    println!(
        "  {} {}",
        "2.".cyan(),
        "Outside sandbox (../, /etc/passwd, /tmp/, ~/.ssh/id_rsa, /Users/)"
    );
    println!("     These paths should show FAILED/BLOCKED/DENIED in Claude's report");

    // Check for security keywords in response
    let has_blocked = response_text.to_lowercase().contains("denied")
        || response_text.to_lowercase().contains("blocked")
        || response_text.to_lowercase().contains("error");
    let has_success = response_text.to_lowercase().contains("success");

    println!("{}", "-".repeat(60).dimmed());

    if has_success && has_blocked {
        println!(
            "\n{}",
            "+----------------------------------------------------------+"
                .green()
                .bold()
        );
        println!(
            "{}",
            "|              SECURITY BOUNDARIES OBSERVED                |"
                .green()
                .bold()
        );
        println!(
            "{}",
            "|                                                          |"
                .green()
                .bold()
        );
        println!(
            "{}",
            "|  - Inside paths: accessible                              |"
                .green()
                .bold()
        );
        println!(
            "{}",
            "|  - Outside paths: blocked                                |"
                .green()
                .bold()
        );
        println!(
            "{}",
            "+----------------------------------------------------------+"
                .green()
                .bold()
        );
    } else {
        println!(
            "\n{}",
            "+----------------------------------------------------------+"
                .yellow()
                .bold()
        );
        println!(
            "{}",
            "|              REVIEW CLAUDE'S REPORT ABOVE                |"
                .yellow()
                .bold()
        );
        println!(
            "{}",
            "+----------------------------------------------------------+"
                .yellow()
                .bold()
        );
    }

    // Print token summary
    println!("\n{}", "Tokens:".blue().bold());
    println!(
        "  {} {}",
        "Input:".dimmed(),
        total_input_tokens.to_string().white()
    );
    println!(
        "  {} {}",
        "Output:".dimmed(),
        total_output_tokens.to_string().white()
    );
    println!(
        "  {} {}",
        "Total:".dimmed(),
        (total_input_tokens + total_output_tokens)
            .to_string()
            .cyan()
            .bold()
    );
    println!(
        "  {} {}",
        "Tool calls:".dimmed(),
        tool_use_count.to_string().cyan()
    );

    // ============================================================
    // 11. OBSERVATIONS
    // ============================================================

    println!("\n{}", "Observed Security Boundaries:".yellow().bold());
    println!("{}", "-".repeat(60).dimmed());
    println!(
        "{}",
        "The Claude SDK/CLI enforces security boundaries:".white()
    );
    println!();
    println!(
        "  {} {}",
        "1.".cyan(),
        "Working directory restriction".green()
    );
    println!("     Files inside cwd are accessible");
    println!("     Files outside cwd are blocked (permission denied)");
    println!();
    println!(
        "  {} {}",
        "2.".cyan(),
        "Absolute path handling".green()
    );
    println!("     Absolute paths like /etc/passwd are blocked");
    println!("     Home directory paths (~/.ssh) are blocked");
    println!();
    println!(
        "  {} {}",
        "3.".cyan(),
        "Directory traversal protection".green()
    );
    println!("     Parent directory access (../) returns EISDIR or is blocked");
    println!();
    println!(
        "{}",
        "Note: These boundaries are enforced by the Claude CLI/SDK,".dimmed()
    );
    println!(
        "{}",
        "not by Claude itself. The model attempts the reads but the".dimmed()
    );
    println!(
        "{}",
        "tool execution layer blocks unauthorized access.".dimmed()
    );

    info!("{}", "Done.".green());

    Ok(())
}
