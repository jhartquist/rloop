use claude_agent_sdk_rs::{ClaudeAgentOptions, ClaudeClient};
use futures::StreamExt;
use tempfile::tempdir;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let project_dir = tempdir()?;
    let builtin_tools = [
        "Task",
        "TaskOutput",
        "Bash",
        "Glob",
        "Grep",
        "ExitPlanMode",
        "Read",
        "Write",
        "Edit",
        "NotebookEdit",
        "WebFetch",
        "TodoWrite",
        "WebSearch",
        "TaskStop",
        "AskUserQuestion",
        "Skill",
        "EnterPlanMode",
        "ToolSearch",
    ]
    .map(String::from)
    .to_vec();

    let options = ClaudeAgentOptions::builder()
        .model("haiku")
        .cwd(project_dir.as_ref())
        .setting_sources(vec![])
        .allowed_tools(vec![])
        .disallowed_tools(builtin_tools)
        .build();

    let mut client = ClaudeClient::new(options);

    client.connect().await?;
    client.query("What is 3 + 42?").await?;

    {
        let mut stream = client.receive_response();
        while let Some(result) = stream.next().await {
            let message = result?;
            dbg!(message);
        }
    }

    client.disconnect().await?;

    Ok(())
}
