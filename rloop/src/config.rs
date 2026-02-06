use std::fs;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;

use crate::model::Model;

pub struct Config {
    pub model: Model,
    pub max_turns: u32,
    pub max_retries: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: Model::Sonnet,
            max_turns: 50,
            max_retries: 10,
        }
    }
}

/// Intermediate representation for partial TOML parsing.
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
}

const DEFAULT_CONFIG_TOML: &str = "\
[step]
model = \"Opus\"
max_turns = 50
max_retries = 10
";

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
    })
}

/// Initialize `.rloop/` directory structure in a repo.
/// Creates directories, default config, adds gitignore entries.
///
/// # Errors
/// Returns an error if directory creation or file writing fails due to permissions or I/O issues.
pub fn init_rloop_dir(repo: impl AsRef<Path>) -> anyhow::Result<PathBuf> {
    let repo = repo.as_ref();
    let rloop_dir = repo.join(".rloop");

    for subdir in &["tasks", "worktrees", "sessions"] {
        fs::create_dir_all(rloop_dir.join(subdir))?;
    }

    let config_path = rloop_dir.join("config.toml");
    if !config_path.exists() {
        fs::write(&config_path, DEFAULT_CONFIG_TOML)?;
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
        assert!(matches!(config.model, Model::Sonnet));
        assert_eq!(config.max_turns, 50);
        assert_eq!(config.max_retries, 10);
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
            "[step]\nmodel = \"Haiku\"\nmax_turns = 30\nmax_retries = 5\n",
        )
        .unwrap();
        let config = load_config(dir.path()).unwrap();
        assert!(matches!(config.model, Model::Haiku));
        assert_eq!(config.max_turns, 30);
        assert_eq!(config.max_retries, 5);
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
}
