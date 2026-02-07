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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: Model::Opus,
            max_turns: 50,
            max_retries: 10,
            timeout_seconds: 300,
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
        };
        Ok(toml::to_string_pretty(&file)?)
    }
}

/// Intermediate representation for partial TOML parsing (input).
#[derive(Deserialize)]
struct ConfigFile {
    #[serde(default)]
    step: Option<StepSection>,
}

#[derive(Deserialize)]
struct StepSection {
    model: Option<Model>,
    max_turns: Option<u32>,
    max_retries: Option<u32>,
    timeout_seconds: Option<u64>,
}

/// Intermediate representation for TOML output.
#[derive(Serialize)]
struct ConfigFileOut {
    step: StepSectionOut,
}

#[derive(Serialize)]
struct StepSectionOut {
    model: Model,
    max_turns: u32,
    max_retries: u32,
    timeout_seconds: u64,
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

    let Some(step) = file.step else {
        return Ok(defaults);
    };

    Ok(Config {
        model: step.model.unwrap_or(defaults.model),
        max_turns: step.max_turns.unwrap_or(defaults.max_turns),
        max_retries: step.max_retries.unwrap_or(defaults.max_retries),
        timeout_seconds: step.timeout_seconds.unwrap_or(defaults.timeout_seconds),
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
        fs::write(&gitignore_path, "worktrees/\nsessions/\n")?;
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
