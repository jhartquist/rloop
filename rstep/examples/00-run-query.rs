use anyhow::Result;
use rstep::{model::Model, query::run_query};
use tempfile::tempdir;

#[tokio::main]
async fn main() -> Result<()> {
    let temp_dir = tempdir()?;

    run_query("What is 2+2?", Model::Haiku, temp_dir.path(), &[])
        .await?
        .print_summary();

    Ok(())
}
