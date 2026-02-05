use std::{collections::HashSet, path::Path};

use anyhow::Result;
use claude_agent_sdk_rs::{ClaudeAgentOptions, ClaudeClient, Message};
use futures::StreamExt;
use strum::{Display, EnumIter, IntoEnumIterator};
use tempfile::tempdir;

#[derive(Debug, Display, EnumIter, PartialEq, Eq, Hash)]
enum BuiltinTool {
    Task,
    TaskOutput,
    Bash,
    Glob,
    Grep,
    ExitPlanMode,
    Read,
    Write,
    Edit,
    NotebookEdit,
    WebFetch,
    TodoWrite,
    WebSearch,
    TaskStop,
    AskUserQuestion,
    Skill,
    EnterPlanMode,
    ToolSearch,
}

#[derive(Debug, Display, Copy, Clone)]
enum Model {
    Haiku,
    Sonnet,
    Opus,
}

#[tokio::main]
async fn main() -> Result<()> {
    let temp_dir = tempdir()?;

    let prompt = "";
    let step = run_step(prompt, Model::Haiku, temp_dir.path()).await?;

    dbg!(step);

    Ok(())
}

#[derive(Debug)]
struct StepSummary {
    pub response: String,
}

async fn run_step(prompt: &str, model: Model, cwd: impl AsRef<Path>) -> Result<StepSummary> {
    let tools: HashSet<BuiltinTool> = BuiltinTool::iter().collect();

    let options = ClaudeAgentOptions::builder()
        .model(model.to_string())
        .cwd(cwd.as_ref())
        .setting_sources(vec![])
        .allowed_tools(vec![])
        .disallowed_tools(tools.iter().map(|t| t.to_string()).collect::<Vec<_>>())
        .build();

    let mut client = ClaudeClient::new(options);

    client.connect().await?;
    client.query(prompt).await?;

    let mut response = String::new();

    {
        let mut stream = client.receive_response();
        while let Some(result) = stream.next().await {
            let message = result?;
            dbg!(&message);

            if let Message::Result(result_message) = message {
                if let Some(result) = result_message.result {
                    response = result.to_string();
                }
            }
        }
    }

    client.disconnect().await?;

    let step_summary = StepSummary { response };

    Ok(step_summary)
}
