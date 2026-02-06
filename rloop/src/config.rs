use std::path::Path;
use std::path::PathBuf;

use crate::model::Model;

pub struct Config {
    pub model: Model,
    pub max_turns: u32,
    pub max_retries: u32,
}

/// Load configuration from `.rloop/config.toml`, merging with defaults.
pub fn load_config(_rloop_dir: &Path) -> anyhow::Result<Config> {
    todo!()
}

/// Initialize `.rloop/` directory structure in a repo.
/// Creates directories, default config, adds gitignore entries.
pub fn init_rloop_dir(_repo: &Path) -> anyhow::Result<PathBuf> {
    todo!()
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
        std::fs::write(
            dir.path().join("config.toml"),
            "[step]\nmodel = \"Opus\"\n",
        )
        .unwrap();
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
        std::fs::write(
            dir.path().join("config.toml"),
            "[step]\nmodel = \"gpt4\"\n",
        )
        .unwrap();
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
