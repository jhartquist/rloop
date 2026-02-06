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

    let prompt = "Read src/lib.rs and describe what the add function does. Is it correct?";

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
