use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::KelvinResult;

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
pub struct ModelProviderHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelProviderProfile {
    pub id: String,
    pub provider_name: String,
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
    pub fn builtin(id: &str) -> Option<Self> {
        match id.trim() {
            OPENAI_RESPONSES_PROFILE_ID => Some(Self {
                id: OPENAI_RESPONSES_PROFILE_ID.to_string(),
                provider_name: "openai".to_string(),
                api_key_env: "OPENAI_API_KEY".to_string(),
                base_url_env: "OPENAI_BASE_URL".to_string(),
                default_base_url: "https://api.openai.com".to_string(),
                endpoint_path: "v1/responses".to_string(),
                auth_header: "authorization".to_string(),
                auth_scheme: ModelProviderAuthScheme::Bearer,
                static_headers: Vec::new(),
                default_allow_hosts: vec!["api.openai.com".to_string()],
            }),
            ANTHROPIC_MESSAGES_PROFILE_ID => Some(Self {
                id: ANTHROPIC_MESSAGES_PROFILE_ID.to_string(),
                provider_name: "anthropic".to_string(),
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
            }),
            _ => None,
        }
    }
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
        ModelProviderAuthScheme, ModelProviderProfile, ANTHROPIC_MESSAGES_PROFILE_ID,
        OPENAI_RESPONSES_PROFILE_ID,
    };

    #[test]
    fn builtin_provider_profiles_cover_openai_and_anthropic() {
        let openai = ModelProviderProfile::builtin(OPENAI_RESPONSES_PROFILE_ID)
            .expect("openai profile should resolve");
        assert_eq!(openai.provider_name, "openai");
        assert_eq!(openai.auth_scheme, ModelProviderAuthScheme::Bearer);
        assert_eq!(openai.default_allow_hosts, vec!["api.openai.com"]);

        let anthropic = ModelProviderProfile::builtin(ANTHROPIC_MESSAGES_PROFILE_ID)
            .expect("anthropic profile should resolve");
        assert_eq!(anthropic.provider_name, "anthropic");
        assert_eq!(anthropic.auth_scheme, ModelProviderAuthScheme::Raw);
        assert_eq!(anthropic.default_allow_hosts, vec!["api.anthropic.com"]);
    }
}
