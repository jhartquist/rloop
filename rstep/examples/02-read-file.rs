use anyhow::Result;
use colored::Colorize;
use rstep::{
    helpers,
    model::Model,
    print::EventPrinter,
    step::run_step_with,
    tool::BuiltinTool,
};
use tempfile::tempdir;

#[tokio::main]
async fn main() -> Result<()> {
    let temp_dir = tempdir()?;

    helpers::write_file(temp_dir.path(), "hello.txt", "Hello, Claude")?;

    let mut printer = EventPrinter::new(temp_dir.path());

    let prompt = "What does hello.txt say?";

    println!("{}  {}", "PROMPT".blue().bold(), prompt);

    run_step_with(
        prompt,
        Model::Haiku,
        temp_dir.path(),
        &[BuiltinTool::Read],
        |msg| printer.print(msg),
    )
    .await?;

    Ok(())
}
