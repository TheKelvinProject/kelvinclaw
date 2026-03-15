use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{KelvinError, KelvinResult};

pub const OPENAI_RESPONSES_PROFILE_ID: &str = "openai.responses";
pub const ANTHROPIC_MESSAGES_PROFILE_ID: &str = "anthropic.messages";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelInput {
    pub run_id: String,
    pub session_id: String,
    pub system_prompt: String,
    pub user_prompt: String,
    pub memory_snippets: Vec<String>,
    pub history: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelOutput {
    pub assistant_text: String,
    pub stop_reason: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<ModelUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelProviderAuthScheme {
    Bearer,
    Raw,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ModelProviderProtocolFamily {
    #[serde(rename = "openai_responses")]
    OpenAiResponses,
    #[serde(rename = "openai_chat_completions")]
    OpenAiChatCompletions,
    #[serde(rename = "anthropic_messages")]
    AnthropicMessages,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelProviderHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelProviderProfile {
    pub id: String,
    pub provider_name: String,
    pub protocol_family: ModelProviderProtocolFamily,
    pub api_key_env: String,
    pub base_url_env: String,
    pub default_base_url: String,
    pub endpoint_path: String,
    pub auth_header: String,
    pub auth_scheme: ModelProviderAuthScheme,
    pub static_headers: Vec<ModelProviderHeader>,
    pub default_allow_hosts: Vec<String>,
}

impl ModelProviderProfile {
    pub fn validate(&self) -> KelvinResult<()> {
        validate_identifier("provider_profile.id", &self.id)?;
        validate_identifier("provider_profile.provider_name", &self.provider_name)?;
        validate_identifier("provider_profile.api_key_env", &self.api_key_env)?;
        validate_identifier("provider_profile.base_url_env", &self.base_url_env)?;
        validate_header_name("provider_profile.auth_header", &self.auth_header)?;
        validate_http_url("provider_profile.default_base_url", &self.default_base_url)?;
        validate_endpoint_path(&self.endpoint_path)?;
        if self.default_allow_hosts.is_empty() {
            return Err(KelvinError::InvalidInput(
                "provider_profile.default_allow_hosts must not be empty".to_string(),
            ));
        }
        for host in &self.default_allow_hosts {
            validate_host_pattern(host)?;
        }
        for header in &self.static_headers {
            validate_header_name("provider_profile.static_headers[].name", &header.name)?;
            if header.value.trim().is_empty() {
                return Err(KelvinError::InvalidInput(
                    "provider_profile.static_headers[].value must not be empty".to_string(),
                ));
            }
        }
        Ok(())
    }

    pub fn builtin(id: &str) -> Option<Self> {
        let profile = match id.trim() {
            OPENAI_RESPONSES_PROFILE_ID => Self {
                id: OPENAI_RESPONSES_PROFILE_ID.to_string(),
                provider_name: "openai".to_string(),
                protocol_family: ModelProviderProtocolFamily::OpenAiResponses,
                api_key_env: "OPENAI_API_KEY".to_string(),
                base_url_env: "OPENAI_BASE_URL".to_string(),
                default_base_url: "https://api.openai.com".to_string(),
                endpoint_path: "v1/responses".to_string(),
                auth_header: "authorization".to_string(),
                auth_scheme: ModelProviderAuthScheme::Bearer,
                static_headers: Vec::new(),
                default_allow_hosts: vec!["api.openai.com".to_string()],
            },
            ANTHROPIC_MESSAGES_PROFILE_ID => Self {
                id: ANTHROPIC_MESSAGES_PROFILE_ID.to_string(),
                provider_name: "anthropic".to_string(),
                protocol_family: ModelProviderProtocolFamily::AnthropicMessages,
                api_key_env: "ANTHROPIC_API_KEY".to_string(),
                base_url_env: "ANTHROPIC_BASE_URL".to_string(),
                default_base_url: "https://api.anthropic.com".to_string(),
                endpoint_path: "v1/messages".to_string(),
                auth_header: "x-api-key".to_string(),
                auth_scheme: ModelProviderAuthScheme::Raw,
                static_headers: vec![ModelProviderHeader {
                    name: "anthropic-version".to_string(),
                    value: "2023-06-01".to_string(),
                }],
                default_allow_hosts: vec!["api.anthropic.com".to_string()],
            },
            _ => return None,
        };
        Some(profile)
    }

    pub fn default_model_name(&self) -> &'static str {
        match self.protocol_family {
            ModelProviderProtocolFamily::OpenAiResponses => "gpt-4.1-mini",
            ModelProviderProtocolFamily::OpenAiChatCompletions => {
                if self.provider_name == "openrouter" {
                    "openai/gpt-4.1-mini"
                } else {
                    "default"
                }
            }
            ModelProviderProtocolFamily::AnthropicMessages => "claude-haiku-4-5-20251001",
        }
    }
}

fn validate_identifier(label: &str, value: &str) -> KelvinResult<()> {
    let value = value.trim();
    if value.is_empty() {
        return Err(KelvinError::InvalidInput(format!(
            "{label} must not be empty"
        )));
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err(KelvinError::InvalidInput(format!(
            "{label} has invalid characters: {value}"
        )));
    }
    Ok(())
}

fn validate_header_name(label: &str, value: &str) -> KelvinResult<()> {
    let value = value.trim();
    if value.is_empty() {
        return Err(KelvinError::InvalidInput(format!(
            "{label} must not be empty"
        )));
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return Err(KelvinError::InvalidInput(format!(
            "{label} has invalid characters: {value}"
        )));
    }
    Ok(())
}

fn validate_http_url(label: &str, value: &str) -> KelvinResult<()> {
    let value = value.trim();
    if !(value.starts_with("https://") || value.starts_with("http://")) {
        return Err(KelvinError::InvalidInput(format!(
            "{label} must start with http:// or https://"
        )));
    }
    if value.contains(char::is_whitespace) {
        return Err(KelvinError::InvalidInput(format!(
            "{label} must not contain whitespace"
        )));
    }
    Ok(())
}

fn validate_endpoint_path(value: &str) -> KelvinResult<()> {
    let value = value.trim();
    if value.is_empty() {
        return Err(KelvinError::InvalidInput(
            "provider_profile.endpoint_path must not be empty".to_string(),
        ));
    }
    if value.starts_with('/') || value.contains("..") {
        return Err(KelvinError::InvalidInput(
            "provider_profile.endpoint_path must be a safe relative path".to_string(),
        ));
    }
    Ok(())
}

fn validate_host_pattern(value: &str) -> KelvinResult<()> {
    let value = value.trim();
    if value.is_empty() {
        return Err(KelvinError::InvalidInput(
            "provider_profile.default_allow_hosts entries must not be empty".to_string(),
        ));
    }
    if value == "*" {
        return Ok(());
    }
    let candidate = value.strip_prefix("*.").unwrap_or(value);
    if !candidate
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-'))
    {
        return Err(KelvinError::InvalidInput(format!(
            "provider_profile.default_allow_hosts has invalid host pattern: {value}"
        )));
    }
    Ok(())
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn provider_name(&self) -> &str;

    fn model_name(&self) -> &str;

    async fn infer(&self, input: ModelInput) -> KelvinResult<ModelOutput>;
}

#[cfg(test)]
mod tests {
    use super::{
        ModelProviderAuthScheme, ModelProviderProfile, ModelProviderProtocolFamily,
        ANTHROPIC_MESSAGES_PROFILE_ID, OPENAI_RESPONSES_PROFILE_ID,
    };

    #[test]
    fn builtin_provider_profiles_cover_openai_and_anthropic() {
        let openai = ModelProviderProfile::builtin(OPENAI_RESPONSES_PROFILE_ID)
            .expect("openai profile should resolve");
        assert_eq!(openai.provider_name, "openai");
        assert_eq!(openai.auth_scheme, ModelProviderAuthScheme::Bearer);
        assert_eq!(
            openai.protocol_family,
            ModelProviderProtocolFamily::OpenAiResponses
        );
        assert_eq!(openai.default_allow_hosts, vec!["api.openai.com"]);
        openai.validate().expect("openai profile should validate");

        let anthropic = ModelProviderProfile::builtin(ANTHROPIC_MESSAGES_PROFILE_ID)
            .expect("anthropic profile should resolve");
        assert_eq!(anthropic.provider_name, "anthropic");
        assert_eq!(anthropic.auth_scheme, ModelProviderAuthScheme::Raw);
        assert_eq!(
            anthropic.protocol_family,
            ModelProviderProtocolFamily::AnthropicMessages
        );
        assert_eq!(anthropic.default_allow_hosts, vec!["api.anthropic.com"]);
        anthropic
            .validate()
            .expect("anthropic profile should validate");
    }
}
