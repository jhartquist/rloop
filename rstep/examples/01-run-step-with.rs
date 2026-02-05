use anyhow::Result;
use rstep::{model::Model, step::run_step_with};
use tempfile::tempdir;

#[tokio::main]
async fn main() -> Result<()> {
    let temp_dir = tempdir()?;

    run_step_with("What is 2+2?", Model::Haiku, temp_dir.path(), &[], |msg| {
        dbg!(msg);
    })
    .await?
    .print_summary();

    Ok(())
}
