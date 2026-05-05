mod helpers;

use rloop::git::{
    assert_clean_worktree, assert_has_commits, cleanup_worktree, cleanup_worktree_preserve_branch,
    create_worktree, fast_forward_ref, lock_worktree, rebase, rebase_abort, unlock_worktree,
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

// --- rebase tests ---

fn git_run(args: &[&str], cwd: &std::path::Path) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn rebase_no_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());

    // Create a branch and add a commit on it
    git_run(&["branch", "feature"], &repo);
    git_run(&["checkout", "feature"], &repo);
    std::fs::write(repo.join("feature.txt"), "feature work\n").unwrap();
    git_run(&["add", "."], &repo);
    git_run(&["commit", "-m", "feature commit"], &repo);
    git_run(&["checkout", "main"], &repo);

    // Add a commit on main (so feature diverges)
    std::fs::write(repo.join("main-change.txt"), "main work\n").unwrap();
    git_run(&["add", "."], &repo);
    git_run(&["commit", "-m", "main commit"], &repo);

    // Checkout feature and rebase onto main
    git_run(&["checkout", "feature"], &repo);
    rebase(&repo, "main").unwrap();
    assert!(repo.join("feature.txt").exists());
    assert!(repo.join("main-change.txt").exists());

    // Feature should be a descendant of main
    let main_rev = git_run(&["rev-parse", "main"], &repo);
    let merge_base = git_run(&["merge-base", "main", "feature"], &repo);
    assert_eq!(
        merge_base, main_rev,
        "feature should be based on main after rebase"
    );
}

#[test]
fn rebase_conflict_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());

    // Create a branch and modify README.md
    git_run(&["branch", "feature"], &repo);
    git_run(&["checkout", "feature"], &repo);
    std::fs::write(repo.join("README.md"), "feature version\n").unwrap();
    git_run(&["add", "."], &repo);
    git_run(&["commit", "-m", "feature change README"], &repo);
    git_run(&["checkout", "main"], &repo);

    // Modify the same file on main to create a conflict
    std::fs::write(repo.join("README.md"), "main version\n").unwrap();
    git_run(&["add", "."], &repo);
    git_run(&["commit", "-m", "main change README"], &repo);

    // Checkout feature and rebase should fail due to conflict
    git_run(&["checkout", "feature"], &repo);
    let result = rebase(&repo, "main");
    assert!(result.is_err(), "rebase should fail on conflict");
}

#[test]
fn rebase_abort_after_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());

    // Create conflicting branches
    git_run(&["branch", "feature"], &repo);
    git_run(&["checkout", "feature"], &repo);
    std::fs::write(repo.join("README.md"), "feature version\n").unwrap();
    git_run(&["add", "."], &repo);
    git_run(&["commit", "-m", "feature change"], &repo);
    git_run(&["checkout", "main"], &repo);

    std::fs::write(repo.join("README.md"), "main version\n").unwrap();
    git_run(&["add", "."], &repo);
    git_run(&["commit", "-m", "main change"], &repo);

    // Checkout feature and attempt rebase (fails)
    git_run(&["checkout", "feature"], &repo);
    let _ = rebase(&repo, "main");

    // Abort should succeed
    rebase_abort(&repo).unwrap();

    // After abort, the repo should be clean (on the rebasing branch)
    // Checkout main to verify the repo is in a good state
    git_run(&["checkout", "main"], &repo);
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md")).unwrap(),
        "main version\n"
    );
}

#[test]
fn rebase_noop_when_already_descendant() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());

    // Create a branch off main and add a commit
    git_run(&["branch", "feature"], &repo);
    git_run(&["checkout", "feature"], &repo);
    std::fs::write(repo.join("feature.txt"), "work\n").unwrap();
    git_run(&["add", "."], &repo);
    git_run(&["commit", "-m", "feature work"], &repo);
    let before = git_run(&["rev-parse", "HEAD"], &repo);

    // Rebase feature onto main — should be a no-op since feature is already based on main
    rebase(&repo, "main").unwrap();

    let after = git_run(&["rev-parse", "HEAD"], &repo);
    assert_eq!(
        before, after,
        "rebase should be a no-op when already a descendant"
    );
}

// --- cleanup_worktree_preserve_branch tests ---

#[test]
fn cleanup_preserve_branch_removes_dir_keeps_branch() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let wt = create_worktree(&repo, TaskId(0), "main").unwrap();
    let wt_path = wt.path.clone();
    let branch = wt.branch.clone();

    cleanup_worktree_preserve_branch(&repo, &wt).unwrap();

    // Directory should be gone
    assert!(!wt_path.exists());

    // Branch should still exist
    let output = std::process::Command::new("git")
        .args(["branch", "--list", &branch])
        .current_dir(&repo)
        .output()
        .unwrap();
    let branches = String::from_utf8_lossy(&output.stdout);
    assert!(
        branches.contains(&branch),
        "branch {branch} should be preserved"
    );
}

#[test]
fn cleanup_preserve_branch_unlocks_first() {
    let dir = tempfile::tempdir().unwrap();
    let repo = helpers::init_git_repo(dir.path());
    let wt = create_worktree(&repo, TaskId(0), "main").unwrap();
    lock_worktree(&wt).unwrap();

    cleanup_worktree_preserve_branch(&repo, &wt).unwrap();
    assert!(!wt.path.exists());
}
