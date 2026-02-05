use anyhow::Result;
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

    run_step_with(
        "Read hello.txt, then edit it to say 'Hello, World' instead. \
         Then write a new file goodbye.txt containing 'Goodbye, Claude'.",
        Model::Haiku,
        temp_dir.path(),
        &[BuiltinTool::Read, BuiltinTool::Write, BuiltinTool::Edit],
        |msg| printer.print(msg),
    )
    .await?;

    Ok(())
}
