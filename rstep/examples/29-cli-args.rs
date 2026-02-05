//! Example 29: CLI Argument Parsing
//!
//! Demonstrates command-line argument parsing with clap for the rstep CLI.
//!
//! This example builds the Args struct that will drive the rstep command:
//!
//!   rstep [TASK_ID] [OPTIONS]
//!
//! Run with:
//!   cargo run --example 29-cli-args -- --help
//!   cargo run --example 29-cli-args -- bd-123 --dry-run -v
//!   cargo run --example 29-cli-args -- --no-merge
//!   cargo run --example 29-cli-args -- -c custom-config.toml bd-456

use clap::Parser;
use colored::Colorize;
use std::path::PathBuf;

// ============================================================
// CLI ARGUMENT DEFINITION
// ============================================================

/// rstep - Single-step task executor
///
/// Run one step of the rloop MDP. Gets a task, executes it in isolation,
/// verifies the result, and merges on success.
#[derive(Parser, Debug)]
#[command(name = "rstep")]
#[command(version, about, long_about = None)]
struct Args {
    /// Task ID to execute (e.g., bd-abc123). If omitted, gets next ready task from br.
    task_id: Option<String>,

    /// Preview execution plan without running
    #[arg(short = 'n', long)]
    dry_run: bool,

    /// Skip merge step (for parallel orchestration)
    #[arg(long)]
    no_merge: bool,

    /// Verbose output
    #[arg(short, long)]
    verbose: bool,

    /// Config file path (default: .rloop/config.toml)
    #[arg(short, long, value_name = "PATH")]
    config: Option<PathBuf>,
}

// ============================================================
// MAIN
// ============================================================

fn main() {
    let args = Args::parse();

    println!();
    println!(
        "{}",
        "+======================================================================+"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|                       CLI ARGUMENT PARSING                           |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|                                                                      |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "|  Demonstrating clap argument parsing for rstep                       |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "+======================================================================+"
            .cyan()
            .bold()
    );

    println!("\n{}", "Parsed Arguments:".yellow().bold());
    println!("{}", "-".repeat(50));

    // Task ID
    match &args.task_id {
        Some(id) => println!("  {} {}", "Task ID:".white(), id.cyan()),
        None => println!(
            "  {} {}",
            "Task ID:".white(),
            "(none - will get from br)".dimmed()
        ),
    }

    // Flags
    println!(
        "  {} {}",
        "Dry run:".white(),
        format_bool(args.dry_run)
    );
    println!(
        "  {} {}",
        "No merge:".white(),
        format_bool(args.no_merge)
    );
    println!(
        "  {} {}",
        "Verbose:".white(),
        format_bool(args.verbose)
    );

    // Config path
    match &args.config {
        Some(path) => println!("  {} {}", "Config:".white(), path.display().to_string().cyan()),
        None => println!(
            "  {} {}",
            "Config:".white(),
            "(default: .rloop/config.toml)".dimmed()
        ),
    }

    println!("{}", "-".repeat(50));

    // Show what would happen
    println!("\n{}", "Execution Plan:".yellow().bold());

    if args.dry_run {
        println!("  {} Would preview execution without running", "1.".dimmed());
    } else {
        // Step 1: Task
        match &args.task_id {
            Some(id) => println!("  {} Execute task: {}", "1.".dimmed(), id.cyan()),
            None => println!(
                "  {} Get next ready task from {}",
                "1.".dimmed(),
                "br".cyan()
            ),
        }

        // Step 2: Config
        match &args.config {
            Some(path) => println!(
                "  {} Load config from: {}",
                "2.".dimmed(),
                path.display().to_string().cyan()
            ),
            None => println!(
                "  {} Load config from: {}",
                "2.".dimmed(),
                ".rloop/config.toml".cyan()
            ),
        }

        // Step 3: Execute
        println!("  {} Create worktree and execute step", "3.".dimmed());

        // Step 4: Verify
        println!("  {} Run verification commands", "4.".dimmed());

        // Step 5: Merge (or not)
        if args.no_merge {
            println!(
                "  {} {} (--no-merge)",
                "5.".dimmed(),
                "Skip merge".yellow()
            );
        } else {
            println!("  {} Merge to main on success", "5.".dimmed());
        }

        // Step 6: Cleanup
        println!("  {} Cleanup worktree", "6.".dimmed());
    }

    // Show debug info if verbose
    if args.verbose {
        println!("\n{}", "Debug Info:".yellow().bold());
        println!("  {:?}", args);
    }

    println!();
    println!(
        "{}",
        "+======================================================================+"
            .green()
            .bold()
    );
    println!(
        "{}",
        "|                    ARGUMENT PARSING COMPLETE                         |"
            .green()
            .bold()
    );
    println!(
        "{}",
        "+======================================================================+"
            .green()
            .bold()
    );
    println!();

    // Show usage examples
    println!("{}", "Example Invocations:".yellow().bold());
    println!();
    println!("  {} Show help", "cargo run --example 29-cli-args -- --help".dimmed());
    println!("  {} Run specific task", "cargo run --example 29-cli-args -- bd-123".dimmed());
    println!("  {} Preview without running", "cargo run --example 29-cli-args -- bd-123 --dry-run".dimmed());
    println!("  {} Verbose mode", "cargo run --example 29-cli-args -- -v".dimmed());
    println!("  {} Skip merge", "cargo run --example 29-cli-args -- --no-merge".dimmed());
    println!("  {} Custom config", "cargo run --example 29-cli-args -- -c ./my-config.toml".dimmed());
    println!("  {} Combined", "cargo run --example 29-cli-args -- bd-123 -n -v --no-merge".dimmed());
    println!();
}

/// Format a boolean as colored YES/NO
fn format_bool(value: bool) -> String {
    if value {
        "YES".green().to_string()
    } else {
        "NO".dimmed().to_string()
    }
}
