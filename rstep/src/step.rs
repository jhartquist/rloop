use std::{
    collections::HashSet,
    fmt,
    path::{Path, PathBuf},
};

use anyhow::Result;
use claude_agent_sdk_rs::{ClaudeAgentOptions, ClaudeClient, Message};
use futures::StreamExt;
use strum::IntoEnumIterator;

use crate::{model::Model, tool::BuiltinTool};

#[derive(Debug)]
pub struct SessionId(pub String);

#[derive(Debug)]
pub struct ModelId(pub String);

#[derive(Debug)]
pub struct StepResult {
    pub session_id: SessionId,
    pub project_dir: PathBuf,
    pub model_id: ModelId,
    pub prompt: String,
    pub response: String,
    pub total_cost_usd: f64,
    pub duration_ms: u64,
}

impl StepResult {
    pub fn print_summary(&self) {
        println!("{self}");
    }
}

impl fmt::Display for StepResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let duration_secs = self.duration_ms as f64 / 1000.0;
        writeln!(f, "── Step Result ────────────────────")?;
        writeln!(f, "  Session:  {}", self.session_id.0)?;
        writeln!(f, "  Model:    {}", self.model_id.0)?;
        writeln!(f, "  Dir:      {}", self.project_dir.display())?;
        writeln!(f, "  Cost:     ${:.6}", self.total_cost_usd)?;
        writeln!(f, "  Duration: {:.2}s", duration_secs)?;
        writeln!(f, "── Prompt ─────────────────────────")?;
        for line in self.prompt.lines() {
            writeln!(f, "  {line}")?;
        }
        writeln!(f, "── Response ───────────────────────")?;
        for line in self.response.lines() {
            writeln!(f, "  {line}")?;
        }
        write!(f, "───────────────────────────────────")
    }
}

pub async fn run_step(
    prompt: &str,
    model: Model,
    project_dir: impl AsRef<Path>,
    tools: &[BuiltinTool],
) -> Result<StepResult> {
    run_step_with(prompt, model, project_dir, tools, |_| {}).await
}

pub async fn run_step_with(
    prompt: &str,
    model: Model,
    project_dir: impl AsRef<Path>,
    tools: &[BuiltinTool],
    mut on_message: impl FnMut(&Message),
) -> Result<StepResult> {
    let allowed_tools: HashSet<BuiltinTool> = tools.iter().copied().collect();
    let disallowed_tools: HashSet<BuiltinTool> = BuiltinTool::iter()
        .collect::<HashSet<_>>()
        .difference(&allowed_tools)
        .copied()
        .collect();

    let options = ClaudeAgentOptions::builder()
        .model(model.to_string())
        .cwd(project_dir.as_ref())
        .setting_sources(vec![])
        .allowed_tools(
            allowed_tools
                .iter()
                .map(|t| t.to_string())
                .collect::<Vec<_>>(),
        )
        .disallowed_tools(
            disallowed_tools
                .iter()
                .map(|t| t.to_string())
                .collect::<Vec<_>>(),
        )
        .build();

    let mut client = ClaudeClient::new(options);

    client.connect().await?;
    client.query(prompt).await?;

    let mut messages = Vec::new();
    {
        let mut stream = client.receive_response();
        while let Some(result) = stream.next().await {
            let message = result?;
            on_message(&message);
            messages.push(message);
        }
    }

    client.disconnect().await?;

    let Some(Message::System(system_message)) = messages.first() else {
        anyhow::bail!("expected first message to be a system message");
    };

    let Some(Message::Result(result_message)) = messages.last() else {
        anyhow::bail!("expected last message to be a result message");
    };

    let session_id = SessionId(
        system_message
            .session_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("missing session_id"))?,
    );

    let model_id = ModelId(
        system_message
            .model
            .clone()
            .ok_or_else(|| anyhow::anyhow!("missing model"))?,
    );

    let response = result_message.result.clone().unwrap_or_default();

    let total_cost_usd = result_message.total_cost_usd.unwrap_or(0.0);
    let duration_ms = result_message.duration_ms;

    Ok(StepResult {
        session_id,
        project_dir: project_dir.as_ref().to_path_buf(),
        model_id,
        prompt: prompt.to_string(),
        response,
        total_cost_usd,
        duration_ms,
    })
}
