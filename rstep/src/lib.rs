pub mod model;
pub mod tool;

use std::path::Path;

use claude_agent_sdk_rs::{ClaudeAgentOptions, ClaudeClient, Message, ResultMessage};
use futures::StreamExt;
use strum::IntoEnumIterator;

use crate::{model::Model, tool::BuiltinTool};

pub async fn query(
    prompt: &str,
    model: Model,
    project_dir: impl AsRef<Path>,
) -> anyhow::Result<String> {
    let builtin_tools: Vec<String> = BuiltinTool::iter().map(|tool| tool.to_string()).collect();

    let options = ClaudeAgentOptions::builder()
        .model(model.to_string())
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
            match &message {
                Message::Assistant(_msg) => println!("[ASST]"),
                Message::User(_msg) => println!("[USER]"),
                Message::System(_msg) => println!("[SYSTEM]"),
                Message::Result(_msg) => println!("[RESULT]"),
                _ => unimplemented!(),
            };
            messages.push(message);
        }
    }

    client.disconnect().await?;

    let Some(Message::Result(ResultMessage {
        result: Some(response),
        ..
    })) = messages.last()
    else {
        anyhow::bail!("expected last message to be a result message");
    };

    Ok(response.to_owned())
}
