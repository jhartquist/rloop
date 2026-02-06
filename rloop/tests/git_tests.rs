mod helpers;

use rloop::git::{
    assert_clean_worktree, assert_has_commits, cleanup_worktree, create_worktree, fast_forward_ref,
    lock_worktree, unlock_worktree,
};
use rloop::task::TaskId;

#[test]
fn create_worktree_creates_dir() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let wt = create_worktree(&repo, TaskId(0), "main").unwrap();
    assert!(wt.path.exists());
    assert!(wt.path.is_dir());
}

#[test]
fn create_worktree_creates_branch() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let wt = create_worktree(&repo, TaskId(1), "main").unwrap();
    assert!(wt.branch.starts_with("rloop-wt-"));
}

#[test]
fn create_worktree_branches_from_base() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let wt = create_worktree(&repo, TaskId(0), "main").unwrap();
    // The worktree should contain files from the base branch
    assert!(wt.path.join("README.md").exists());
}

#[test]
fn create_worktree_returns_correct_fields() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let wt = create_worktree(&repo, TaskId(5), "main").unwrap();
    assert_eq!(wt.task_id, TaskId(5));
    assert!(!wt.branch.is_empty());
    assert!(wt.path.exists());
}

#[test]
fn lock_prevents_removal() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let wt = create_worktree(&repo, TaskId(0), "main").unwrap();
    lock_worktree(&wt).unwrap();
    // Locked worktree: `git worktree remove` should fail
    let output = std::process::Command::new("git")
        .args(["worktree", "remove", wt.path.to_str().unwrap()])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn unlock_allows_removal() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let wt = create_worktree(&repo, TaskId(0), "main").unwrap();
    lock_worktree(&wt).unwrap();
    unlock_worktree(&wt).unwrap();
    // Now removal should succeed
    let output = std::process::Command::new("git")
        .args(["worktree", "remove", wt.path.to_str().unwrap()])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(output.status.success());
}

#[test]
fn fast_forward_ref_moves_branch() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());

    // Create a session branch to fast-forward
    let run = |args: &[&str], cwd: &std::path::Path| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    run(&["branch", "rloop-00"], &repo);

    let wt = create_worktree(&repo, TaskId(0), "rloop-00").unwrap();

    // Make a change in the worktree
    std::fs::write(wt.path.join("new_file.txt"), "hello from worktree\n").unwrap();
    run(&["add", "."], &wt.path);
    run(&["commit", "-m", "worktree change"], &wt.path);

    let wt_tip = run(&["rev-parse", "HEAD"], &wt.path);

    fast_forward_ref(&repo, "rloop-00", &wt.branch).unwrap();

    // Verify session branch points at worktree tip
    let session_tip = run(&["rev-parse", "rloop-00"], &repo);
    assert_eq!(session_tip, wt_tip);
}

#[test]
fn cleanup_removes_dir_and_branch() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let wt = create_worktree(&repo, TaskId(0), "main").unwrap();
    let wt_path = wt.path.clone();
    let branch = wt.branch.clone();

    cleanup_worktree(&repo, &wt).unwrap();

    assert!(!wt_path.exists());
    // Branch should be deleted
    let output = std::process::Command::new("git")
        .args(["branch", "--list", &branch])
        .current_dir(&repo)
        .output()
        .unwrap();
    let branches = String::from_utf8_lossy(&output.stdout);
    assert!(!branches.contains(&branch));
}

#[test]
fn cleanup_unlocks_first() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let wt = create_worktree(&repo, TaskId(0), "main").unwrap();
    lock_worktree(&wt).unwrap();
    // cleanup should handle the unlock internally
    cleanup_worktree(&repo, &wt).unwrap();
    assert!(!wt.path.exists());
}

#[test]
fn full_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());

    // Create session branch and worktree
    let run = |args: &[&str], cwd: &std::path::Path| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    run(&["branch", "rloop-03"], &repo);

    let wt = create_worktree(&repo, TaskId(3), "rloop-03").unwrap();
    assert!(wt.path.exists());

    // Lock
    lock_worktree(&wt).unwrap();

    // Work in worktree
    std::fs::write(wt.path.join("feature.rs"), "fn feature() {}\n").unwrap();
    run(&["add", "."], &wt.path);
    run(&["commit", "-m", "add feature"], &wt.path);

    // Unlock
    unlock_worktree(&wt).unwrap();

    // Fast-forward session branch
    fast_forward_ref(&repo, "rloop-03", &wt.branch).unwrap();

    // Cleanup
    cleanup_worktree(&repo, &wt).unwrap();
    assert!(!wt.path.exists());

    // Verify session branch has the commit
    let log = run(&["log", "--oneline", "rloop-03"], &repo);
    assert!(log.contains("add feature"));
}

#[test]
fn assert_has_commits_yes() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    assert_has_commits(&repo).unwrap();
}

#[test]
fn assert_has_commits_empty() {
    let dir = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(assert_has_commits(dir.path()).is_err());
}

#[test]
fn assert_clean_worktree_clean() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    assert_clean_worktree(&repo).unwrap();
}

#[test]
fn assert_clean_worktree_dirty() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    std::fs::write(repo.join("dirty.txt"), "uncommitted\n").unwrap();
    assert!(assert_clean_worktree(&repo).is_err());
}
