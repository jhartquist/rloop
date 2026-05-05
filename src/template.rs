use std::path::Path;
use std::process::Command;

use anyhow::Context;
use tempfile::TempDir;

/// Available project templates for examples and tests.
pub enum Template {
    /// Single-function crate with a bug in `add` (returns `a - b` instead of `a + b`).
    BugAdder,
    /// Two-function crate with bugs in both `add` and `multiply`.
    TwoBugs,
}

impl Template {
    fn dir_name(&self) -> &str {
        match self {
            Self::BugAdder => "bug-adder",
            Self::TwoBugs => "two-bugs",
        }
    }
}

/// Copy a template into a new temp directory and initialize it as a git repo.
///
/// Skips `target/` directories when copying. Returns the `TempDir` guard —
/// the directory is cleaned up when the guard is dropped.
///
/// # Errors
/// Returns an error if the template directory is missing, copying fails, or git init fails.
pub fn copy_template(template: &Template) -> anyhow::Result<TempDir> {
    let templates_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
    let src = templates_root.join(template.dir_name());

    anyhow::ensure!(src.is_dir(), "template not found: {}", src.display());

    let tmp = TempDir::new().context("failed to create temp dir")?;
    copy_dir_skip_target(&src, tmp.path())?;
    init_git(tmp.path())?;

    Ok(tmp)
}

/// Recursively copy `src` into `dst`, skipping any directory named `target`.
fn copy_dir_skip_target(src: &Path, dst: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let name = entry.file_name();

        if file_type.is_dir() {
            if name == "target" {
                continue;
            }
            copy_dir_skip_target(&entry.path(), &dst.join(&name))?;
        } else {
            std::fs::copy(entry.path(), dst.join(&name))?;
        }
    }
    Ok(())
}

fn init_git(dir: &Path) -> anyhow::Result<()> {
    let run = |args: &[&str]| -> anyhow::Result<()> {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .context("failed to run git")?;
        anyhow::ensure!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    };

    run(&["init"])?;
    run(&["config", "user.email", "test@test.com"])?;
    run(&["config", "user.name", "Test"])?;
    run(&["add", "."])?;
    run(&["commit", "-m", "init"])?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_bug_adder() {
        let dir = copy_template(&Template::BugAdder).unwrap();
        assert!(dir.path().join("Cargo.toml").exists());
        assert!(dir.path().join("src/lib.rs").exists());
        assert!(!dir.path().join("target").exists());
    }

    #[test]
    fn copy_two_bugs() {
        let dir = copy_template(&Template::TwoBugs).unwrap();
        assert!(dir.path().join("Cargo.toml").exists());
        assert!(dir.path().join("src/lib.rs").exists());
        assert!(!dir.path().join("target").exists());
    }

    #[test]
    fn template_is_git_repo() {
        let dir = copy_template(&Template::BugAdder).unwrap();
        assert!(dir.path().join(".git").exists());
    }
}
