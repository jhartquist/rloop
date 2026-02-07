use std::fs;
use std::path::Path;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::Model;

pub struct Config {
    pub model: Model,
    pub max_turns: u32,
    pub max_retries: u32,
    /// Timeout in seconds for each verification command. `0` means no timeout.
    pub timeout_seconds: u64,
    pub task: TaskConfig,
    /// Maximum number of tasks to execute in parallel. `1` means sequential.
    pub max_parallel: usize,
}

/// Configuration for `rloop task` subcommands.
pub struct TaskConfig {
    /// Model used by `rloop task new` for the enhancement LLM query.
    pub enhance_model: Model,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: Model::Opus,
            max_turns: 50,
            max_retries: 10,
            timeout_seconds: 300,
            task: TaskConfig::default(),
            max_parallel: 1,
        }
    }
}

impl Default for TaskConfig {
    fn default() -> Self {
        Self {
            enhance_model: Model::Sonnet,
        }
    }
}

impl Config {
    /// Serialize this config to TOML format.
    ///
    /// # Errors
    /// Returns an error if TOML serialization fails.
    pub fn to_toml(&self) -> anyhow::Result<String> {
        let file = ConfigFileOut {
            step: StepSectionOut {
                model: self.model,
                max_turns: self.max_turns,
                max_retries: self.max_retries,
                timeout_seconds: self.timeout_seconds,
            },
            task: TaskSectionOut {
                enhance_model: self.task.enhance_model,
            },
            session: SessionSectionOut {
                max_parallel: self.max_parallel,
            },
        };
        Ok(toml::to_string_pretty(&file)?)
    }
}

/// Intermediate representation for partial TOML parsing (input).
#[derive(Deserialize)]
struct ConfigFile {
    #[serde(default)]
    step: Option<StepSection>,
    #[serde(default)]
    task: Option<TaskSection>,
    #[serde(default)]
    session: Option<SessionSection>,
}

#[derive(Deserialize)]
struct StepSection {
    model: Option<Model>,
    max_turns: Option<u32>,
    max_retries: Option<u32>,
    timeout_seconds: Option<u64>,
}

#[derive(Deserialize)]
struct TaskSection {
    enhance_model: Option<Model>,
}

#[derive(Deserialize)]
struct SessionSection {
    max_parallel: Option<usize>,
}

/// Intermediate representation for TOML output.
#[derive(Serialize)]
struct ConfigFileOut {
    step: StepSectionOut,
    task: TaskSectionOut,
    session: SessionSectionOut,
}

#[derive(Serialize)]
struct StepSectionOut {
    model: Model,
    max_turns: u32,
    max_retries: u32,
    timeout_seconds: u64,
}

#[derive(Serialize)]
struct TaskSectionOut {
    enhance_model: Model,
}

#[derive(Serialize)]
struct SessionSectionOut {
    max_parallel: usize,
}

/// Load configuration from `.rloop/config.toml`, merging with defaults.
///
/// # Errors
/// Returns an error if the config file exists but contains invalid TOML or unrecognized values.
pub fn load_config(rloop_dir: impl AsRef<Path>) -> anyhow::Result<Config> {
    let rloop_dir = rloop_dir.as_ref();
    let config_path = rloop_dir.join("config.toml");

    if !config_path.exists() {
        return Ok(Config::default());
    }

    let contents = fs::read_to_string(&config_path)?;
    let file: ConfigFile = toml::from_str(&contents)?;
    let defaults = Config::default();

    let task_config = match file.task {
        Some(task) => TaskConfig {
            enhance_model: task.enhance_model.unwrap_or(defaults.task.enhance_model),
        },
        None => defaults.task,
    };

    let max_parallel = file
        .session
        .and_then(|s| s.max_parallel)
        .unwrap_or(defaults.max_parallel);

    let Some(step) = file.step else {
        return Ok(Config {
            task: task_config,
            max_parallel,
            ..defaults
        });
    };

    Ok(Config {
        model: step.model.unwrap_or(defaults.model),
        max_turns: step.max_turns.unwrap_or(defaults.max_turns),
        max_retries: step.max_retries.unwrap_or(defaults.max_retries),
        timeout_seconds: step.timeout_seconds.unwrap_or(defaults.timeout_seconds),
        task: task_config,
        max_parallel,
    })
}

/// Initialize `.rloop/` directory structure in a repo.
/// Creates directories, default config, `.gitignore`, and commits the scaffolding.
///
/// # Errors
/// Returns an error if directory creation, file writing, or git commands fail.
pub fn init_rloop_dir(repo: impl AsRef<Path>) -> anyhow::Result<PathBuf> {
    let repo = repo.as_ref();
    let rloop_dir = repo.join(".rloop");

    for subdir in &["tasks", "worktrees", "sessions"] {
        fs::create_dir_all(rloop_dir.join(subdir))?;
    }

    let config_path = rloop_dir.join("config.toml");
    if !config_path.exists() {
        fs::write(&config_path, Config::default().to_toml()?)?;
    }

    let gitignore_path = rloop_dir.join(".gitignore");
    if !gitignore_path.exists() {
        fs::write(&gitignore_path, "worktrees/\nsessions/\n.lock\n")?;
    }

    // Commit scaffolding (idempotent — only commits if there are staged changes).
    // These git commands may fail if the directory is not a git repo, which is fine.
    if let Err(e) = crate::git::git(repo, &["add", ".rloop/config.toml", ".rloop/.gitignore"]) {
        eprintln!("warning: failed to stage rloop scaffolding: {e}");
    }
    let status = crate::git::git(repo, &["status", "--porcelain"]).unwrap_or_default();
    if !status.is_empty()
        && let Err(e) = crate::git::git(repo, &["commit", "-m", "rloop: init"])
    {
        eprintln!("warning: failed to commit rloop scaffolding: {e}");
    }

    Ok(rloop_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;

    #[test]
    fn load_defaults_when_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let config = load_config(dir.path()).unwrap();
        assert!(matches!(config.model, Model::Opus));
        assert_eq!(config.max_turns, 50);
        assert_eq!(config.max_retries, 10);
        assert_eq!(config.timeout_seconds, 300);
        assert!(matches!(config.task.enhance_model, Model::Sonnet));
        assert_eq!(config.max_parallel, 1);
    }

    #[test]
    fn load_partial_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[step]\nmodel = \"Opus\"\n").unwrap();
        let config = load_config(dir.path()).unwrap();
        assert!(matches!(config.model, Model::Opus));
        assert_eq!(config.max_turns, 50); // default
    }

    #[test]
    fn load_full_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[step]\nmodel = \"Haiku\"\nmax_turns = 30\nmax_retries = 5\ntimeout_seconds = 60\n",
        )
        .unwrap();
        let config = load_config(dir.path()).unwrap();
        assert!(matches!(config.model, Model::Haiku));
        assert_eq!(config.max_turns, 30);
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.timeout_seconds, 60);
    }

    #[test]
    fn load_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "not valid toml {{{}}}").unwrap();
        assert!(load_config(dir.path()).is_err());
    }

    #[test]
    fn load_invalid_model() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[step]\nmodel = \"gpt4\"\n").unwrap();
        assert!(load_config(dir.path()).is_err());
    }

    #[test]
    fn init_creates_directory_structure() {
        let dir = tempfile::tempdir().unwrap();
        let rloop_dir = init_rloop_dir(dir.path()).unwrap();
        assert!(rloop_dir.join("tasks").is_dir());
        assert!(rloop_dir.join("worktrees").is_dir());
        assert!(rloop_dir.join("sessions").is_dir());
        assert!(rloop_dir.join(".gitignore").exists());
    }

    #[test]
    fn init_creates_default_config() {
        let dir = tempfile::tempdir().unwrap();
        let rloop_dir = init_rloop_dir(dir.path()).unwrap();
        assert!(rloop_dir.join("config.toml").exists());
    }

    #[test]
    fn init_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let first = init_rloop_dir(dir.path()).unwrap();
        let second = init_rloop_dir(dir.path()).unwrap();
        assert_eq!(first, second);
        assert!(second.join("config.toml").exists());
    }

    #[test]
    fn to_toml_roundtrip() {
        let config = Config::default();
        let toml_str = config.to_toml().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), &toml_str).unwrap();
        let loaded = load_config(dir.path()).unwrap();
        assert!(matches!(loaded.model, Model::Opus));
        assert_eq!(loaded.max_turns, 50);
        assert_eq!(loaded.max_retries, 10);
        assert_eq!(loaded.timeout_seconds, 300);
        assert!(matches!(loaded.task.enhance_model, Model::Sonnet));
        assert_eq!(loaded.max_parallel, 1);
    }

    #[test]
    fn load_task_section() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[task]\nenhance_model = \"Haiku\"\n",
        )
        .unwrap();
        let config = load_config(dir.path()).unwrap();
        assert!(matches!(config.task.enhance_model, Model::Haiku));
        // step defaults still apply
        assert!(matches!(config.model, Model::Opus));
    }

    #[test]
    fn load_task_section_defaults_enhance_model() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[task]\n").unwrap();
        let config = load_config(dir.path()).unwrap();
        assert!(matches!(config.task.enhance_model, Model::Sonnet));
    }

    #[test]
    fn load_both_sections() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[step]\nmodel = \"Haiku\"\n\n[task]\nenhance_model = \"Opus\"\n",
        )
        .unwrap();
        let config = load_config(dir.path()).unwrap();
        assert!(matches!(config.model, Model::Haiku));
        assert!(matches!(config.task.enhance_model, Model::Opus));
    }

    #[test]
    fn init_creates_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        let rloop_dir = init_rloop_dir(dir.path()).unwrap();
        let content = std::fs::read_to_string(rloop_dir.join(".gitignore")).unwrap();
        assert!(content.contains("worktrees/"));
        assert!(content.contains("sessions/"));
    }

    #[test]
    fn load_session_section() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[session]\nmax_parallel = 4\n",
        )
        .unwrap();
        let config = load_config(dir.path()).unwrap();
        assert_eq!(config.max_parallel, 4);
        // step defaults still apply
        assert!(matches!(config.model, Model::Opus));
    }

    #[test]
    fn load_session_section_defaults_max_parallel() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[session]\n").unwrap();
        let config = load_config(dir.path()).unwrap();
        assert_eq!(config.max_parallel, 1);
    }

    #[test]
    fn load_all_three_sections() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[step]\nmodel = \"Haiku\"\n\n[task]\nenhance_model = \"Opus\"\n\n[session]\nmax_parallel = 3\n",
        )
        .unwrap();
        let config = load_config(dir.path()).unwrap();
        assert!(matches!(config.model, Model::Haiku));
        assert!(matches!(config.task.enhance_model, Model::Opus));
        assert_eq!(config.max_parallel, 3);
    }

    #[test]
    fn to_toml_contains_session_section() {
        let config = Config {
            max_parallel: 2,
            ..Config::default()
        };
        let toml_str = config.to_toml().unwrap();
        assert!(toml_str.contains("[session]"));
        assert!(toml_str.contains("max_parallel = 2"));
    }

    #[test]
    fn to_toml_roundtrip_max_parallel() {
        let config = Config {
            max_parallel: 5,
            ..Config::default()
        };
        let toml_str = config.to_toml().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), &toml_str).unwrap();
        let loaded = load_config(dir.path()).unwrap();
        assert_eq!(loaded.max_parallel, 5);
    }

    #[test]
    fn init_commits_scaffolding() {
        use std::process::Command;

        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();

        // Set up a git repo with initial commit
        let run = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(repo)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };

        run(&["init"]);
        run(&["config", "user.email", "test@test.com"]);
        run(&["config", "user.name", "Test"]);
        std::fs::write(repo.join("README.md"), "# test\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-m", "initial"]);

        init_rloop_dir(repo).unwrap();

        // Verify a commit was made with the scaffolding
        let log = run(&["log", "--oneline"]);
        assert!(log.contains("rloop: init"));
    }
}
