use anyhow::Result;
use colored::Colorize;
use rstep::{helpers, model::Model, print::EventPrinter, step::run_step_with, tool::BuiltinTool};
use tempfile::tempdir;

#[tokio::main]
async fn main() -> Result<()> {
    let temp_dir = tempdir()?;

    helpers::write_file(temp_dir.path(), "hello.txt", "Hello, Claude")?;

    let mut printer = EventPrinter::new(temp_dir.path());

    let prompt = "Read hello.txt, then edit it to say 'Hello, World' instead. \
         Then write a new file goodbye.txt containing 'Goodbye, Claude'. \
         Finally, cat goodbye.txt to confirm it was written correctly.";

    println!("{}  {}\n", "PROMPT".blue().bold(), prompt);

    run_step_with(
        prompt,
        Model::Haiku,
        temp_dir.path(),
        &[
            BuiltinTool::Read,
            BuiltinTool::Write,
            BuiltinTool::Edit,
            BuiltinTool::Bash,
        ],
        |msg| printer.print(msg),
    )
    .await?;

    Ok(())
}
