use anyhow::Result;
use rstep::{model::Model, step::run_step};
use tempfile::tempdir;

#[tokio::main]
async fn main() -> Result<()> {
    let temp_dir = tempdir()?;

    run_step("What is 2+2?", Model::Haiku, temp_dir.path(), &[])
        .await?
        .print_summary();

    Ok(())
}
