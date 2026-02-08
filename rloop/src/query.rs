use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context;
use claude_agent_sdk_rs::types::config::SettingSource;
use claude_agent_sdk_rs::types::messages::ContentBlock;
use claude_agent_sdk_rs::{ClaudeAgentOptions, ClaudeClient, Message};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;

use crate::log::Event;
use crate::model::Model;
use crate::step::get_u64;
use crate::tool::BuiltinTool;

/// Configuration for a lightweight LLM query.
pub struct QueryConfig {
    pub model: Model,
    pub cwd: PathBuf,
    pub tools: Vec<BuiltinTool>,
    pub output_format: Option<serde_json::Value>,
}

/// Result of a lightweight LLM query.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueryResult {
    pub response: String,
    pub structured_output: Option<serde_json::Value>,
    pub cost_usd: f64,
    #[serde(with = "crate::step::duration_serde")]
    pub duration: Duration,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub events: Vec<Event>,
}

/// Compute allowed/disallowed tool lists from the requested tools.
fn compute_tool_lists(tools: &[BuiltinTool]) -> (Vec<String>, Vec<String>) {
    let allowed: HashSet<BuiltinTool> = tools.iter().copied().collect();
    let disallowed: HashSet<BuiltinTool> = BuiltinTool::iter()
        .collect::<HashSet<_>>()
        .difference(&allowed)
        .copied()
        .collect();

    let allowed: Vec<String> = allowed.iter().map(ToString::to_string).collect();
    let disallowed: Vec<String> = disallowed.iter().map(ToString::to_string).collect();

    (allowed, disallowed)
}

/// Extract a `TokenUsage` event from a Claude SDK usage JSON value.
fn extract_token_usage(usage: &serde_json::Value) -> Event {
    Event::TokenUsage {
        input_tokens: get_u64(usage, "input_tokens"),
        output_tokens: get_u64(usage, "output_tokens"),
        cache_read_input_tokens: get_u64(usage, "cache_read_input_tokens"),
        cache_creation_input_tokens: get_u64(usage, "cache_creation_input_tokens"),
    }
}

/// Collected data from processing the query response stream.
struct StreamData {
    response: String,
    structured_output: Option<serde_json::Value>,
    cost_usd: f64,
    input_tokens: u64,
    output_tokens: u64,
    events: Vec<Event>,
}

/// Process the Claude response stream silently, collecting all events.
async fn process_stream(client: &mut ClaudeClient) -> anyhow::Result<StreamData> {
    let mut events: Vec<Event> = Vec::new();
    let mut response = String::new();
    let mut structured_output: Option<serde_json::Value> = None;
    let mut cost_usd = 0.0;
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;

    let mut stream = client.receive_response();
    while let Some(result) = stream.next().await {
        let msg = result.context("error receiving message from Claude stream")?;

        match &msg {
            Message::Assistant(m) => {
                for block in &m.message.content {
                    match block {
                        ContentBlock::Text(t) => {
                            events.push(Event::AssistantMessage {
                                content: t.text.clone(),
                            });
                        }
                        ContentBlock::ToolUse(tu) => {
                            events.push(Event::ToolCall {
                                name: tu.name.clone(),
                                input: tu.input.clone(),
                            });
                        }
                        _ => {}
                    }
                }

                if let Some(usage) = &m.message.usage {
                    events.push(extract_token_usage(usage));
                }
            }
            Message::User(user) => {
                let tool_use_id = user
                    .extra
                    .pointer("/message/content/0/tool_use_id")
                    .and_then(|v| v.as_str());

                let tur = user.extra.get("tool_use_result").cloned();
                let content_text = user
                    .extra
                    .pointer("/message/content/0/content")
                    .and_then(|v| v.as_str())
                    .map(String::from);

                if tur.is_some() || content_text.is_some() {
                    let name = tool_use_id.unwrap_or("?").to_string();
                    let output = tur
                        .or_else(|| content_text.map(serde_json::Value::String))
                        .unwrap_or(serde_json::Value::Null);
                    events.push(Event::ToolResult { name, output });
                }
            }
            Message::Result(res) => {
                response = res.result.clone().unwrap_or_default();
                structured_output.clone_from(&res.structured_output);
                cost_usd = res.total_cost_usd.unwrap_or(0.0);

                if let Some(usage) = &res.usage {
                    input_tokens = get_u64(usage, "input_tokens");
                    output_tokens = get_u64(usage, "output_tokens");
                    events.push(extract_token_usage(usage));
                }
            }
            _ => {}
        }
    }

    Ok(StreamData {
        response,
        structured_output,
        cost_usd,
        input_tokens,
        output_tokens,
        events,
    })
}

/// Run a lightweight LLM query — no MCP tools, no interruption, no retry.
///
/// Runs silently (no terminal output), collects all stream events internally,
/// and returns them in the result for the caller to log as needed.
///
/// # Errors
/// Returns an error if client connection, query, or stream processing fails.
pub async fn run_query(prompt: &str, config: &QueryConfig) -> anyhow::Result<QueryResult> {
    let start = Instant::now();

    let (allowed_tools, disallowed_tools) = compute_tool_lists(&config.tools);

    let options = if let Some(ref schema) = config.output_format {
        ClaudeAgentOptions::builder()
            .model(config.model.to_string())
            .cwd(&config.cwd)
            .setting_sources(vec![SettingSource::Project])
            .allowed_tools(allowed_tools)
            .disallowed_tools(disallowed_tools)
            .output_format(schema.clone())
            .build()
    } else {
        ClaudeAgentOptions::builder()
            .model(config.model.to_string())
            .cwd(&config.cwd)
            .setting_sources(vec![SettingSource::Project])
            .allowed_tools(allowed_tools)
            .disallowed_tools(disallowed_tools)
            .build()
    };

    let mut client = ClaudeClient::new(options);
    client
        .connect()
        .await
        .context("failed to connect to Claude")?;
    client
        .query(prompt)
        .await
        .context("failed to send prompt to Claude")?;

    let data = process_stream(&mut client).await?;

    if let Err(e) = client.disconnect().await {
        eprintln!("warning: failed to disconnect Claude client: {e}");
    }

    Ok(QueryResult {
        response: data.response,
        structured_output: data.structured_output,
        cost_usd: data.cost_usd,
        duration: start.elapsed(),
        input_tokens: data.input_tokens,
        output_tokens: data.output_tokens,
        events: data.events,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_config_defaults() {
        let config = QueryConfig {
            model: Model::Sonnet,
            cwd: PathBuf::from("/tmp/test"),
            tools: vec![],
            output_format: None,
        };
        assert_eq!(config.model, Model::Sonnet);
        assert!(config.tools.is_empty());
        assert!(config.output_format.is_none());
    }

    #[test]
    fn query_config_with_tools() {
        let config = QueryConfig {
            model: Model::Haiku,
            cwd: PathBuf::from("/tmp/test"),
            tools: vec![BuiltinTool::Read, BuiltinTool::Glob, BuiltinTool::Grep],
            output_format: None,
        };
        assert_eq!(config.tools.len(), 3);
    }

    #[test]
    fn query_config_with_structured_output() {
        let schema = serde_json::json!({
            "type": "json_schema",
            "schema": {
                "type": "object",
                "properties": {
                    "answer": { "type": "string" }
                },
                "required": ["answer"]
            }
        });
        let config = QueryConfig {
            model: Model::Sonnet,
            cwd: PathBuf::from("/tmp/test"),
            tools: vec![],
            output_format: Some(schema.clone()),
        };
        assert_eq!(config.output_format.unwrap(), schema);
    }

    #[test]
    fn query_result_serialization() {
        let result = QueryResult {
            response: "test response".into(),
            structured_output: None,
            cost_usd: 0.05,
            duration: Duration::from_secs(10),
            input_tokens: 1000,
            output_tokens: 500,
            events: vec![],
        };
        let json = serde_json::to_string(&result).unwrap();
        let roundtripped: QueryResult = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtripped.response, "test response");
        assert_eq!(roundtripped.input_tokens, 1000);
        assert_eq!(roundtripped.output_tokens, 500);
    }

    #[test]
    fn query_result_with_structured_output() {
        let output = serde_json::json!({"answer": "42"});
        let result = QueryResult {
            response: String::new(),
            structured_output: Some(output.clone()),
            cost_usd: 0.01,
            duration: Duration::from_secs(5),
            input_tokens: 200,
            output_tokens: 50,
            events: vec![],
        };
        assert_eq!(result.structured_output.unwrap(), output);
    }

    #[test]
    fn query_result_with_events() {
        let result = QueryResult {
            response: "done".into(),
            structured_output: None,
            cost_usd: 0.02,
            duration: Duration::from_secs(3),
            input_tokens: 500,
            output_tokens: 100,
            events: vec![
                Event::AssistantMessage {
                    content: "hello".into(),
                },
                Event::ToolCall {
                    name: "Read".into(),
                    input: serde_json::json!({"file_path": "/tmp/test.rs"}),
                },
            ],
        };
        assert_eq!(result.events.len(), 2);
    }

    #[test]
    fn compute_tool_lists_empty() {
        let (allowed, disallowed) = compute_tool_lists(&[]);
        assert!(allowed.is_empty());
        assert!(!disallowed.is_empty());
    }

    #[test]
    fn compute_tool_lists_read_only() {
        let (allowed, disallowed) = compute_tool_lists(&[BuiltinTool::Read]);
        assert_eq!(allowed.len(), 1);
        assert!(allowed.contains(&"Read".to_string()));
        assert!(!disallowed.contains(&"Read".to_string()));
    }

    #[test]
    fn compute_tool_lists_multiple() {
        let tools = vec![BuiltinTool::Read, BuiltinTool::Glob, BuiltinTool::Grep];
        let (allowed, disallowed) = compute_tool_lists(&tools);
        assert_eq!(allowed.len(), 3);
        for tool in &tools {
            assert!(allowed.contains(&tool.to_string()));
            assert!(!disallowed.contains(&tool.to_string()));
        }
    }
}
