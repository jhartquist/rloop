//! Example 15: Worktree Create
//!
//! Demonstrates creating a git worktree for isolated task execution.
//!
//! What this does:
//! 1. Creates a temp directory
//! 2. Initializes a git repo with the bug-adder template
//! 3. Makes an initial commit on main
//! 4. Creates a worktree: `git worktree add .rloop/worktrees/{task-id} -b rloop/{task-id}`
//! 5. Verifies the worktree exists and has the correct branch
//! 6. Verifies main branch is untouched
//! 7. Cleans up
//!
//! Run with:
//!   cargo run --example 15-worktree-create
//!
//! Key additions from previous examples:
//!   - Pure git operations (no Claude involved)
//!   - Worktree creation for isolated branches
//!   - Task isolation pattern for parallel execution

use anyhow::{Context, Result};
use colored::Colorize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
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

/// Run a git command in the given directory, returning stdout
fn git(repo_path: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .output()
        .with_context(|| format!("Failed to run git {:?}", args))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git {:?} failed: {}", args, stderr);
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Initialize a git repo with initial commit
fn init_repo(repo_path: &Path) -> Result<()> {
    git(repo_path, &["init"])?;
    git(repo_path, &["config", "user.email", "test@example.com"])?;
    git(repo_path, &["config", "user.name", "Test User"])?;

    // Add all files and create initial commit
    git(repo_path, &["add", "-A"])?;
    git(repo_path, &["commit", "-m", "Initial commit"])?;

    Ok(())
}

/// Create a worktree for the given task ID
fn create_worktree(repo_path: &Path, task_id: &str) -> Result<PathBuf> {
    let worktree_dir = repo_path.join(".rloop/worktrees");
    fs::create_dir_all(&worktree_dir)?;

    let worktree_path = worktree_dir.join(task_id);
    let branch_name = format!("rloop/{}", task_id);

    git(
        repo_path,
        &[
            "worktree",
            "add",
            &worktree_path.to_string_lossy(),
            "-b",
            &branch_name,
        ],
    )?;

    Ok(worktree_path)
}

/// Get the current branch name
fn current_branch(repo_path: &Path) -> Result<String> {
    git(repo_path, &["branch", "--show-current"])
}

/// List all branches
fn list_branches(repo_path: &Path) -> Result<Vec<String>> {
    let output = git(repo_path, &["branch", "-a"])?;
    Ok(output
        .lines()
        .map(|line| line.trim().trim_start_matches("* ").to_string())
        .collect())
}

/// List all worktrees
fn list_worktrees(repo_path: &Path) -> Result<String> {
    git(repo_path, &["worktree", "list"])
}

/// Get the last commit hash (short)
fn last_commit(repo_path: &Path) -> Result<String> {
    git(repo_path, &["rev-parse", "--short", "HEAD"])
}

/// Remove a worktree
fn remove_worktree(repo_path: &Path, worktree_path: &Path) -> Result<()> {
    git(
        repo_path,
        &["worktree", "remove", &worktree_path.to_string_lossy()],
    )?;
    Ok(())
}

/// Delete a branch
fn delete_branch(repo_path: &Path, branch_name: &str) -> Result<()> {
    git(repo_path, &["branch", "-d", branch_name])?;
    Ok(())
}

// ============================================================
// MAIN
// ============================================================

fn main() -> Result<()> {
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
        "|                  WORKTREE CREATE EXAMPLE                 |"
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
        "|  Creating isolated worktree for task execution           |"
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "+----------------------------------------------------------+"
            .cyan()
            .bold()
    );

    // ============================================================
    // 1. CREATE TEMP DIRECTORY
    // ============================================================

    let temp = tempdir()?;
    let repo_path = temp.path();
    info!("{}", "Setting up temp directory...".cyan().bold());
    println!(
        "\n{} {}",
        "Temp directory:".yellow().bold(),
        repo_path.display().to_string().white()
    );

    // ============================================================
    // 2. COPY TEMPLATE INTO TEMP DIRECTORY
    // ============================================================

    let template_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates/bug-adder");
    copy_dir_all(&template_dir, repo_path)?;
    info!(
        "{} {}",
        "Copied template:".green(),
        "bug-adder".white().bold()
    );

    // ============================================================
    // 3. INITIALIZE GIT REPO
    // ============================================================

    println!("\n{}", "Initializing git repository...".yellow().bold());
    init_repo(repo_path)?;

    let main_branch = current_branch(repo_path)?;
    let initial_commit = last_commit(repo_path)?;
    println!(
        "  {} {}",
        "Branch:".dimmed(),
        main_branch.green().bold()
    );
    println!(
        "  {} {}",
        "Initial commit:".dimmed(),
        initial_commit.white()
    );

    // ============================================================
    // 4. CREATE WORKTREE
    // ============================================================

    let task_id = "bd-abc123";
    println!(
        "\n{} {}",
        "Creating worktree for task:".yellow().bold(),
        task_id.cyan().bold()
    );

    let worktree_path = create_worktree(repo_path, task_id)?;
    let branch_name = format!("rloop/{}", task_id);

    println!(
        "  {} {}",
        "Worktree path:".dimmed(),
        worktree_path.display().to_string().white()
    );
    println!(
        "  {} {}",
        "Branch name:".dimmed(),
        branch_name.cyan()
    );

    // ============================================================
    // 5. VERIFY WORKTREE STRUCTURE
    // ============================================================

    println!("\n{}", "Verifying worktree structure...".yellow().bold());

    // Show worktree list
    let worktrees = list_worktrees(repo_path)?;
    println!("\n{}", "Git worktree list:".green().bold());
    println!("{}", "-".repeat(60).dimmed());
    for line in worktrees.lines() {
        println!("  {}", line);
    }
    println!("{}", "-".repeat(60).dimmed());

    // Verify worktree exists
    if worktree_path.exists() {
        println!(
            "\n{} {}",
            "Worktree directory exists:".green(),
            "YES".green().bold()
        );
    } else {
        anyhow::bail!("Worktree directory does not exist!");
    }

    // Verify worktree has correct branch
    let wt_branch = current_branch(&worktree_path)?;
    println!(
        "  {} {}",
        "Worktree branch:".dimmed(),
        wt_branch.cyan().bold()
    );

    if wt_branch == branch_name {
        println!(
            "  {} {}",
            "Branch correct:".green(),
            "YES".green().bold()
        );
    } else {
        anyhow::bail!(
            "Worktree has wrong branch: expected {}, got {}",
            branch_name,
            wt_branch
        );
    }

    // Verify worktree has the same files
    let wt_has_cargo = worktree_path.join("Cargo.toml").exists();
    let wt_has_lib = worktree_path.join("src/lib.rs").exists();
    println!(
        "  {} {}",
        "Has Cargo.toml:".dimmed(),
        if wt_has_cargo { "YES".green() } else { "NO".red() }
    );
    println!(
        "  {} {}",
        "Has src/lib.rs:".dimmed(),
        if wt_has_lib { "YES".green() } else { "NO".red() }
    );

    // ============================================================
    // 6. VERIFY MAIN BRANCH IS UNTOUCHED
    // ============================================================

    println!("\n{}", "Verifying main branch...".yellow().bold());

    // Check main repo is still on main branch
    let main_current = current_branch(repo_path)?;
    println!(
        "  {} {}",
        "Main repo branch:".dimmed(),
        main_current.green().bold()
    );

    if main_current == main_branch {
        println!(
            "  {} {}",
            "Main untouched:".green(),
            "YES".green().bold()
        );
    } else {
        anyhow::bail!(
            "Main repo switched branches: expected {}, got {}",
            main_branch,
            main_current
        );
    }

    // Show all branches
    let branches = list_branches(repo_path)?;
    println!("\n{}", "All branches:".green().bold());
    for branch in &branches {
        let marker = if branch == &branch_name { " (worktree)" } else { "" };
        println!("  - {}{}", branch, marker.dimmed());
    }

    // ============================================================
    // 7. SHOW WORKTREE STRUCTURE
    // ============================================================

    println!("\n{}", "Worktree structure:".blue().bold());
    println!("{}", "=".repeat(60).cyan());
    println!();
    println!(
        "{}",
        format!("{}/", repo_path.display()).white().bold()
    );
    println!("  |-- Cargo.toml");
    println!("  |-- src/");
    println!("  |   |-- lib.rs");
    println!("  |-- .git/              {}", "(main repo)".dimmed());
    println!("  |-- .rloop/");
    println!("      |-- worktrees/");
    println!(
        "          |-- {}/",
        task_id.cyan()
    );
    println!("              |-- Cargo.toml");
    println!("              |-- src/");
    println!("              |   |-- lib.rs");
    println!(
        "              |-- .git       {}",
        "(worktree link)".dimmed()
    );
    println!();
    println!("{}", "=".repeat(60).cyan());

    // ============================================================
    // 8. DEMONSTRATE CLEANUP
    // ============================================================

    println!("\n{}", "Cleaning up worktree...".yellow().bold());

    // Remove worktree
    remove_worktree(repo_path, &worktree_path)?;
    println!(
        "  {} {}",
        "Worktree removed:".green(),
        "YES".green().bold()
    );

    // Delete the branch
    delete_branch(repo_path, &branch_name)?;
    println!(
        "  {} {}",
        "Branch deleted:".green(),
        "YES".green().bold()
    );

    // Verify cleanup
    let worktrees_after = list_worktrees(repo_path)?;
    let branches_after = list_branches(repo_path)?;

    println!("\n{}", "After cleanup:".green().bold());
    println!("{}", "-".repeat(60).dimmed());
    println!("{}", "  Worktrees:");
    for line in worktrees_after.lines() {
        println!("    {}", line);
    }
    println!("{}", "  Branches:");
    for branch in &branches_after {
        println!("    - {}", branch);
    }
    println!("{}", "-".repeat(60).dimmed());

    // ============================================================
    // 9. SUMMARY
    // ============================================================

    println!(
        "\n{}",
        "+----------------------------------------------------------+"
            .green()
            .bold()
    );
    println!(
        "{}",
        "|                  WORKTREE LIFECYCLE COMPLETE             |"
            .green()
            .bold()
    );
    println!(
        "{}",
        "+----------------------------------------------------------+"
            .green()
            .bold()
    );

    println!("\n{}", "Worktree lifecycle:".blue().bold());
    println!("{}", "-".repeat(60).dimmed());
    println!(
        "  {} {}",
        "1. CREATE".cyan(),
        "git worktree add .rloop/worktrees/{task-id} -b rloop/{task-id}"
    );
    println!(
        "  {} {}",
        "2. WORK  ".cyan(),
        "Isolated branch for task, main untouched"
    );
    println!(
        "  {} {}",
        "3. VERIFY".cyan(),
        "Run tests in worktree directory"
    );
    println!(
        "  {} {}",
        "4. MERGE ".cyan(),
        "git merge --squash from main repo"
    );
    println!(
        "  {} {}",
        "5. REMOVE".cyan(),
        "git worktree remove .rloop/worktrees/{task-id}"
    );
    println!(
        "  {} {}",
        "6. DELETE".cyan(),
        "git branch -d rloop/{task-id}"
    );
    println!("{}", "-".repeat(60).dimmed());

    println!("\n{}", "Key benefits:".yellow().bold());
    println!(
        "  {} {}",
        "-".cyan(),
        "Task isolation: each task has its own branch"
    );
    println!(
        "  {} {}",
        "-".cyan(),
        "Parallel execution: multiple worktrees can run simultaneously"
    );
    println!(
        "  {} {}",
        "-".cyan(),
        "Clean rollback: just delete the worktree and branch"
    );
    println!(
        "  {} {}",
        "-".cyan(),
        "Main untouched: main branch stays stable until merge"
    );

    info!("{}", "Done.".green());

    // Temp directory auto-deleted on drop
    Ok(())
}
