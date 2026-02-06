use rstep::query;
use tempfile::tempdir;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let project_dir = tempdir()?;

    let prompt = "What is 3 + 42?";
    println!("Prompt: {prompt}");

    let response = query(prompt, "haiku", project_dir.path()).await?;
    println!("Response: {response}");

    Ok(())
}
