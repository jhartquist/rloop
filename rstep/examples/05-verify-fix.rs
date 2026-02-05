use std::process::Command;

use anyhow::{bail, Result};
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

    run_step_with(
        "Read src/lib.rs, find the bug, and fix it using Edit.",
        Model::Haiku,
        temp_dir.path(),
        &[BuiltinTool::Read, BuiltinTool::Edit],
        |msg| printer.print(msg),
    )
    .await?;

    let output = Command::new("cargo")
        .arg("test")
        .current_dir(temp_dir.path())
        .output()?;

    if output.status.success() {
        println!("{}", "PASS".green().bold());
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!("{}", "FAIL".red().bold());
        for line in stderr.lines() {
            println!("  {line}");
        }
        bail!("cargo test failed");
    }

    Ok(())
}
