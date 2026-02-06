use anyhow::Result;
use colored::Colorize;
use rstep::{
    helpers::{self, Template},
    model::Model,
    print::EventPrinter,
    step::run_step_with,
    tool::BuiltinTool,
};

#[tokio::main]
async fn main() -> Result<()> {
    let temp_dir = helpers::copy_template(Template::BugAdder)?;

    let mut printer = EventPrinter::new(temp_dir.path());

    let prompt = "Read src/lib.rs, find the bug, fix it with Edit, \
         then run `cargo test` to verify your fix.";

    println!("{}  {}\n", "PROMPT".blue().bold(), prompt);

    run_step_with(
        prompt,
        Model::Haiku,
        temp_dir.path(),
        &[BuiltinTool::Read, BuiltinTool::Edit, BuiltinTool::Bash],
        |msg| printer.print(msg),
    )
    .await?;

    Ok(())
}
