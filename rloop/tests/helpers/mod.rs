#![allow(dead_code)]
use std::path::{Path, PathBuf};

/// Write a task markdown file and return the path.
pub fn create_task_file(dir: &Path, id: u32, content: &str) -> PathBuf {
    let filename = format!("{id:02}.md");
    let path = dir.join(&filename);
    std::fs::write(&path, content).unwrap();
    path
}

/// Generate a minimal task file: just id + body.
pub fn minimal_task(id: u32, description: &str) -> String {
    format!("---\nid: {id}\n---\n{description}\n")
}

/// Generate a complete task file with all fields.
pub fn full_task(
    id: u32,
    model: &str,
    deps: &[u32],
    verification: Option<&[&str]>,
    completed: bool,
    description: &str,
) -> String {
    let deps_str = deps
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let ver_str = match verification {
        Some(cmds) => {
            let items = cmds
                .iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(", ");
            format!("verification: [{items}]\n")
        }
        None => String::new(),
    };
    format!(
        "---\nid: {id}\nmodel: {model}\ndepends_on: [{deps_str}]\n{ver_str}completed: {completed}\n---\n{description}\n"
    )
}

/// Commit all files in a repo (git add . && git commit).
pub fn commit_all(repo: &Path, message: &str) {
    use std::process::Command;

    let run = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .expect("git command failed");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    };

    run(&["add", "."]);
    run(&["commit", "-m", message]);
}

/// Initialize a real git repo in the given directory with an initial commit.
pub fn init_git_repo(dir: &Path) -> PathBuf {
    use std::process::Command;

    let run = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git command failed");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    };

    run(&["init"]);
    run(&["config", "user.email", "test@test.com"]);
    run(&["config", "user.name", "Test"]);

    // Create an initial commit (git worktree requires at least one commit)
    std::fs::write(dir.join("README.md"), "# test repo\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-m", "initial commit"]);

    dir.to_path_buf()
}
