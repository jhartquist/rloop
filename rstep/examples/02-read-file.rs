use anyhow::Result;
use rstep::{model::Model, print::EventPrinter, step::run_step_with, tool::BuiltinTool};
use tempfile::tempdir;

#[tokio::main]
async fn main() -> Result<()> {
    let temp_dir = tempdir()?;

    let filename = temp_dir.path().join("hello.txt");
    std::fs::write(&filename, "Hello, Claude")?;

    let mut printer = EventPrinter::new(temp_dir.path());

    run_step_with(
        "What does hello.txt say?",
        Model::Haiku,
        temp_dir.path(),
        &[BuiltinTool::Read],
        |msg| printer.print(msg),
    )
    .await?;

    Ok(())
}
