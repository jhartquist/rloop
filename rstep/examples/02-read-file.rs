use anyhow::Result;
use rstep::{model::Model, step::run_step, tool::BuiltinTool};
use tempfile::tempdir;

#[tokio::main]
async fn main() -> Result<()> {
    let temp_dir = tempdir()?;

    let filename = temp_dir.path().join("hello.txt");
    std::fs::write(&filename, "Hello, Claude")?;

    run_step(
        "What does hello.txt say?",
        Model::Haiku,
        temp_dir.path(),
        &[BuiltinTool::Read],
    )
    .await?
    .print_summary();

    Ok(())
}
