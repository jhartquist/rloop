use std::path::{Path, PathBuf};

use crate::model::Model;

pub struct Config {
    pub model: Model,
    pub max_turns: u32,
    pub max_retries: u32,
    pub session_dir: PathBuf,
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
