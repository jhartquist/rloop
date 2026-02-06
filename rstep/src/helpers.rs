use std::path::{Path, PathBuf};

use anyhow::Result;
use tempfile::TempDir;
use tracing_subscriber::EnvFilter;

pub enum Template {
    BugAdder,
    TwoBugs,
}

impl Template {
    fn dir_name(&self) -> &str {
        match self {
            Template::BugAdder => "bug-adder",
            Template::TwoBugs => "two-bugs",
        }
    }
}

/// Copy a template into a fresh temp directory, returning the TempDir.
pub fn copy_template(template: Template) -> Result<TempDir> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("templates")
        .join(template.dir_name());
    let tmp = tempfile::tempdir()?;
    copy_dir_all(&src, tmp.path())?;
    Ok(tmp)
}

/// Recursively copy a directory and all its contents.
pub fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dest = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

/// Write a file into a directory, returning the full path.
pub fn write_file(dir: &Path, name: &str, content: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    std::fs::write(&path, content)?;
    Ok(path)
}

/// Init tracing with INFO default, overridable via RUST_LOG.
pub fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
}
