use std::path::Path;

use claude_agent_sdk_rs::{ClaudeAgentOptions, ClaudeClient, Message, ResultMessage};
use futures::StreamExt;

pub async fn query(
    prompt: &str,
    model: &str,
    project_dir: impl AsRef<Path>,
) -> anyhow::Result<String> {
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
        .model(model)
        .cwd(project_dir.as_ref())
        .setting_sources(vec![])
        .allowed_tools(vec![])
        .disallowed_tools(builtin_tools)
        .build();

    let mut client = ClaudeClient::new(options);

    client.connect().await?;
    client.query(prompt).await?;

    let mut messages = vec![];
    {
        let mut stream = client.receive_response();
        while let Some(result) = stream.next().await {
            let message = result?;
            messages.push(message);
        }
    }

    client.disconnect().await?;

    let Some(Message::Result(ResultMessage { result: Some(response), .. })) = messages.last() else {
        anyhow::bail!("expected last message to be a result message");
    };

    Ok(response.to_owned())
}
