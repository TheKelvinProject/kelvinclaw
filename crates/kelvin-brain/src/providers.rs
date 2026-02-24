use async_trait::async_trait;
use serde_json::{json, Value};

use kelvin_core::{KelvinResult, ModelInput, ModelOutput, ModelProvider, ModelUsage, ToolCall};

#[derive(Debug, Clone)]
pub struct EchoModelProvider {
    provider: String,
    model: String,
}

impl EchoModelProvider {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }

    fn parse_tool_calls(prompt: &str) -> Vec<ToolCall> {
        let mut calls = Vec::new();
        let mut cursor = prompt;
        let mut idx = 0usize;

        while let Some(start) = cursor.find("[[tool:") {
            let rest = &cursor[start + "[[tool:".len()..];
            let Some(end) = rest.find("]]" ) else {
                break;
            };
            let body = rest[..end].trim();
            let mut split = body.splitn(2, char::is_whitespace);
            let name = split.next().unwrap_or("" ).trim();
            if name.is_empty() {
                cursor = &rest[end + 2..];
                continue;
            }
            let arguments = split
                .next()
                .map(str::trim)
                .and_then(|candidate| {
                    if candidate.is_empty() {
                        None
                    } else {
                        serde_json::from_str::<Value>(candidate).ok()
                    }
                })
                .unwrap_or_else(|| json!({}));

            idx += 1;
            calls.push(ToolCall {
                id: format!("tool-{idx}"),
                name: name.to_string(),
                arguments,
            });
            cursor = &rest[end + 2..];
        }

        calls
    }
}

#[async_trait]
impl ModelProvider for EchoModelProvider {
    fn provider_name(&self) -> &str {
        &self.provider
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    async fn infer(&self, input: ModelInput) -> KelvinResult<ModelOutput> {
        let tool_calls = Self::parse_tool_calls(&input.user_prompt);
        let memory_context = if input.memory_snippets.is_empty() {
            String::new()
        } else {
            let preview = input
                .memory_snippets
                .iter()
                .take(2)
                .map(|item| format!("- {item}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!("\n\nMemory recall:\n{preview}")
        };

        let assistant_text = if tool_calls.is_empty() {
            format!("Echo: {}{}", input.user_prompt.trim(), memory_context)
        } else {
            format!(
                "Received {} tool request(s). Executing requested tools next.",
                tool_calls.len()
            )
        };

        let token_estimate = (input.user_prompt.len() + assistant_text.len()) as u64 / 4;
        let usage = ModelUsage {
            input_tokens: Some((input.user_prompt.len() as u64 / 4).max(1)),
            output_tokens: Some((assistant_text.len() as u64 / 4).max(1)),
            total_tokens: Some(token_estimate.max(1)),
        };

        Ok(ModelOutput {
            assistant_text,
            stop_reason: Some(if tool_calls.is_empty() {
                "completed".to_string()
            } else {
                "tool_calls".to_string()
            }),
            tool_calls,
            usage: Some(usage),
        })
    }
}
