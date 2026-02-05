use anyhow::Result;
use rstep::{
    helpers::{self, Template},
    model::Model,
    print::EventPrinter,
    step::run_step_with,
    tool::BuiltinTool,
};

#[tokio::main]
async fn main() -> Result<()> {
    helpers::init_tracing();
    let temp_dir = helpers::copy_template(Template::BugAdder)?;

    let mut printer = EventPrinter::new(temp_dir.path());

    run_step_with(
        "Read src/lib.rs and describe what the add function does. Is it correct?",
        Model::Haiku,
        temp_dir.path(),
        &[BuiltinTool::Read],
        |msg| printer.print(msg),
    )
    .await?;

    Ok(())
}
