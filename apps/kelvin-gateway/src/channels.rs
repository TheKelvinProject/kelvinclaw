use std::collections::{HashMap, HashSet, VecDeque};

use kelvin_core::{now_ms, KelvinError, RunOutcome};
use kelvin_sdk::{KelvinSdkRunRequest, KelvinSdkRuntime};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::time::{sleep, Duration};
use url::Url;

#[derive(Debug, Clone)]
pub struct ChannelEngine {
    telegram: Option<TelegramChannelAdapter>,
}

impl ChannelEngine {
    pub fn from_env() -> KelvinErrorOr<Self> {
        let config = TelegramChannelConfig::from_env()?;
        let telegram = if config.enabled {
            Some(TelegramChannelAdapter::new(config)?)
        } else {
            None
        };
        Ok(Self { telegram })
    }

    pub async fn telegram_ingest(
        &mut self,
        runtime: &KelvinSdkRuntime,
        request: TelegramIngressRequest,
    ) -> KelvinErrorOr<Value> {
        let Some(adapter) = self.telegram.as_mut() else {
            return Err(KelvinError::NotFound(
                "telegram channel is not enabled".to_string(),
            ));
        };
        adapter.ingest(runtime, request).await
    }

    pub fn telegram_approve_pairing(&mut self, code: &str) -> KelvinErrorOr<Value> {
        let Some(adapter) = self.telegram.as_mut() else {
            return Err(KelvinError::NotFound(
                "telegram channel is not enabled".to_string(),
            ));
        };
        adapter.approve_pairing(code)
    }

    pub fn telegram_status(&self) -> Value {
        if let Some(adapter) = &self.telegram {
            adapter.status()
        } else {
            json!({
                "enabled": false,
            })
        }
    }
}

type KelvinErrorOr<T> = Result<T, KelvinError>;

#[derive(Debug, Clone)]
struct TelegramChannelConfig {
    enabled: bool,
    bot_token: Option<String>,
    api_base_url: String,
    allow_chat_ids: HashSet<i64>,
    pairing_enabled: bool,
    max_messages_per_minute: usize,
    max_seen_delivery_ids: usize,
    outbound_max_retries: u8,
    outbound_retry_backoff_ms: u64,
}

impl TelegramChannelConfig {
    fn from_env() -> KelvinErrorOr<Self> {
        let enabled = read_env_bool("KELVIN_TELEGRAM_ENABLED", false)?;
        let bot_token = std::env::var("KELVIN_TELEGRAM_BOT_TOKEN")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let api_base_url = std::env::var("KELVIN_TELEGRAM_API_BASE_URL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "https://api.telegram.org".to_string());
        let allow_custom_base = read_env_bool("KELVIN_TELEGRAM_ALLOW_CUSTOM_BASE_URL", false)?;
        let parsed_base = Url::parse(&api_base_url).map_err(|err| {
            KelvinError::InvalidInput(format!("invalid KELVIN_TELEGRAM_API_BASE_URL: {err}"))
        })?;
        if parsed_base.scheme() != "https" {
            return Err(KelvinError::InvalidInput(
                "KELVIN_TELEGRAM_API_BASE_URL must use https".to_string(),
            ));
        }
        if !parsed_base.username().is_empty() || parsed_base.password().is_some() {
            return Err(KelvinError::InvalidInput(
                "KELVIN_TELEGRAM_API_BASE_URL must not include credentials".to_string(),
            ));
        }
        if parsed_base.query().is_some() || parsed_base.fragment().is_some() {
            return Err(KelvinError::InvalidInput(
                "KELVIN_TELEGRAM_API_BASE_URL must not include query or fragment".to_string(),
            ));
        }
        let host = parsed_base
            .host_str()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !allow_custom_base && host != "api.telegram.org" {
            return Err(KelvinError::InvalidInput(
                "telegram api base url host must be api.telegram.org unless KELVIN_TELEGRAM_ALLOW_CUSTOM_BASE_URL=true"
                    .to_string(),
            ));
        }

        let allow_chat_ids = std::env::var("KELVIN_TELEGRAM_ALLOW_CHAT_IDS")
            .ok()
            .map(|value| parse_chat_id_set(&value))
            .transpose()?
            .unwrap_or_default();
        let pairing_enabled = read_env_bool("KELVIN_TELEGRAM_PAIRING_ENABLED", true)?;
        let max_messages_per_minute =
            read_env_usize("KELVIN_TELEGRAM_MAX_MESSAGES_PER_MINUTE", 20, 1, 2_000)?;
        let max_seen_delivery_ids =
            read_env_usize("KELVIN_TELEGRAM_MAX_SEEN_DELIVERY_IDS", 4_096, 128, 100_000)?;
        let outbound_max_retries = read_env_u8("KELVIN_TELEGRAM_OUTBOUND_MAX_RETRIES", 2, 0, 10)?;
        let outbound_retry_backoff_ms =
            read_env_u64("KELVIN_TELEGRAM_OUTBOUND_RETRY_BACKOFF_MS", 200, 1, 20_000)?;

        if enabled && bot_token.is_none() {
            eprintln!(
                "warning: telegram channel enabled without bot token; outbound telegram delivery disabled"
            );
        }

        Ok(Self {
            enabled,
            bot_token,
            api_base_url,
            allow_chat_ids,
            pairing_enabled,
            max_messages_per_minute,
            max_seen_delivery_ids,
            outbound_max_retries,
            outbound_retry_backoff_ms,
        })
    }
}

fn parse_chat_id_set(raw: &str) -> KelvinErrorOr<HashSet<i64>> {
    let mut out = HashSet::new();
    for item in raw.split(',') {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        let chat_id = trimmed.parse::<i64>().map_err(|_| {
            KelvinError::InvalidInput(format!("invalid telegram chat id in allow list: {trimmed}"))
        })?;
        out.insert(chat_id);
    }
    Ok(out)
}

fn read_env_bool(name: &str, default: bool) -> KelvinErrorOr<bool> {
    let Ok(value) = std::env::var(name) else {
        return Ok(default);
    };
    match value.trim().to_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(KelvinError::InvalidInput(format!(
            "invalid boolean value for {name}: {value}"
        ))),
    }
}

fn read_env_usize(name: &str, default: usize, min: usize, max: usize) -> KelvinErrorOr<usize> {
    let Ok(value) = std::env::var(name) else {
        return Ok(default);
    };
    let parsed = value.trim().parse::<usize>().map_err(|_| {
        KelvinError::InvalidInput(format!("invalid numeric value for {name}: {value}"))
    })?;
    if parsed < min || parsed > max {
        return Err(KelvinError::InvalidInput(format!(
            "{name} must be between {min} and {max}"
        )));
    }
    Ok(parsed)
}

fn read_env_u8(name: &str, default: u8, min: u8, max: u8) -> KelvinErrorOr<u8> {
    let Ok(value) = std::env::var(name) else {
        return Ok(default);
    };
    let parsed = value.trim().parse::<u8>().map_err(|_| {
        KelvinError::InvalidInput(format!("invalid numeric value for {name}: {value}"))
    })?;
    if parsed < min || parsed > max {
        return Err(KelvinError::InvalidInput(format!(
            "{name} must be between {min} and {max}"
        )));
    }
    Ok(parsed)
}

fn read_env_u64(name: &str, default: u64, min: u64, max: u64) -> KelvinErrorOr<u64> {
    let Ok(value) = std::env::var(name) else {
        return Ok(default);
    };
    let parsed = value.trim().parse::<u64>().map_err(|_| {
        KelvinError::InvalidInput(format!("invalid numeric value for {name}: {value}"))
    })?;
    if parsed < min || parsed > max {
        return Err(KelvinError::InvalidInput(format!(
            "{name} must be between {min} and {max}"
        )));
    }
    Ok(parsed)
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramIngressRequest {
    pub delivery_id: String,
    pub chat_id: i64,
    pub text: String,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone)]
struct TelegramChannelAdapter {
    config: TelegramChannelConfig,
    paired_chat_ids: HashSet<i64>,
    pending_pair_codes: HashMap<String, i64>,
    seen_delivery_ids: HashSet<String>,
    seen_delivery_order: VecDeque<String>,
    rate_windows: HashMap<i64, VecDeque<u128>>,
    client: reqwest::Client,
}

impl TelegramChannelAdapter {
    fn new(config: TelegramChannelConfig) -> KelvinErrorOr<Self> {
        Ok(Self {
            config,
            paired_chat_ids: HashSet::new(),
            pending_pair_codes: HashMap::new(),
            seen_delivery_ids: HashSet::new(),
            seen_delivery_order: VecDeque::new(),
            rate_windows: HashMap::new(),
            client: reqwest::Client::new(),
        })
    }

    fn status(&self) -> Value {
        json!({
            "enabled": true,
            "pairing_enabled": self.config.pairing_enabled,
            "paired_chats": self.paired_chat_ids.len(),
            "pending_pairings": self.pending_pair_codes.len(),
            "seen_delivery_ids": self.seen_delivery_ids.len(),
            "allowlist_size": self.config.allow_chat_ids.len(),
            "outbound_delivery_enabled": self.config.bot_token.is_some(),
        })
    }

    fn approve_pairing(&mut self, code: &str) -> KelvinErrorOr<Value> {
        let normalized = code.trim();
        if normalized.is_empty() {
            return Err(KelvinError::InvalidInput(
                "pairing code must not be empty".to_string(),
            ));
        }
        let Some(chat_id) = self.pending_pair_codes.remove(normalized) else {
            return Err(KelvinError::NotFound("pairing code not found".to_string()));
        };
        self.paired_chat_ids.insert(chat_id);
        Ok(json!({
            "approved": true,
            "chat_id": chat_id,
        }))
    }

    async fn ingest(
        &mut self,
        runtime: &KelvinSdkRuntime,
        request: TelegramIngressRequest,
    ) -> KelvinErrorOr<Value> {
        let delivery_id = request.delivery_id.trim().to_string();
        if delivery_id.is_empty() {
            return Err(KelvinError::InvalidInput(
                "delivery_id must not be empty".to_string(),
            ));
        }
        let text = request.text.trim().to_string();
        if text.is_empty() {
            return Err(KelvinError::InvalidInput(
                "telegram message text must not be empty".to_string(),
            ));
        }
        if text.len() > 4_096 {
            return Err(KelvinError::InvalidInput(
                "telegram message text exceeds 4096 bytes".to_string(),
            ));
        }

        if self.is_duplicate_delivery(&delivery_id) {
            return Ok(json!({
                "status": "deduped",
                "delivery_id": delivery_id,
            }));
        }

        self.enforce_rate_limit(request.chat_id)?;
        if let Some(code) = self.enforce_pairing_policy(request.chat_id)? {
            let pairing_message = format!(
                "KelvinClaw pairing required. Approve code: {code} using channel.telegram.pair.approve."
            );
            let _ = self
                .send_message_with_retry(request.chat_id, &pairing_message)
                .await;
            return Ok(json!({
                "status": "pairing_required",
                "chat_id": request.chat_id,
                "pairing_code": code,
            }));
        }

        self.track_delivery_id(delivery_id.clone());
        let accepted = runtime
            .submit(KelvinSdkRunRequest {
                prompt: text,
                session_id: Some(format!("telegram:{}", request.chat_id)),
                workspace_dir: None,
                timeout_ms: request.timeout_ms,
                system_prompt: None,
                memory_query: None,
                run_id: None,
            })
            .await?;
        let outcome = runtime
            .wait_for_outcome(
                &accepted.run_id,
                request.timeout_ms.unwrap_or(30_000).saturating_add(3_000),
            )
            .await?;

        match outcome {
            RunOutcome::Completed(result) => {
                let response_text = result
                    .payloads
                    .iter()
                    .map(|payload| payload.text.clone())
                    .collect::<Vec<_>>()
                    .join("\n");
                let outbound_text = if response_text.trim().is_empty() {
                    "No response generated.".to_string()
                } else {
                    response_text
                };
                self.send_message_with_retry(request.chat_id, &outbound_text)
                    .await?;

                Ok(json!({
                    "status": "completed",
                    "run_id": accepted.run_id,
                    "delivery_id": delivery_id,
                    "response_text": outbound_text,
                }))
            }
            RunOutcome::Failed(error) => {
                let outbound_text = format!("Kelvin run failed: {error}");
                let _ = self
                    .send_message_with_retry(request.chat_id, &outbound_text)
                    .await;
                Ok(json!({
                    "status": "failed",
                    "run_id": accepted.run_id,
                    "delivery_id": delivery_id,
                    "error": error,
                }))
            }
            RunOutcome::Timeout => {
                let outbound_text = "Kelvin run timed out.".to_string();
                let _ = self
                    .send_message_with_retry(request.chat_id, &outbound_text)
                    .await;
                Ok(json!({
                    "status": "timeout",
                    "run_id": accepted.run_id,
                    "delivery_id": delivery_id,
                }))
            }
        }
    }

    fn is_duplicate_delivery(&self, delivery_id: &str) -> bool {
        self.seen_delivery_ids.contains(delivery_id)
    }

    fn track_delivery_id(&mut self, delivery_id: String) {
        if self.seen_delivery_ids.insert(delivery_id.clone()) {
            self.seen_delivery_order.push_back(delivery_id);
        }
        while self.seen_delivery_order.len() > self.config.max_seen_delivery_ids {
            if let Some(evicted) = self.seen_delivery_order.pop_front() {
                self.seen_delivery_ids.remove(&evicted);
            }
        }
    }

    fn enforce_rate_limit(&mut self, chat_id: i64) -> KelvinErrorOr<()> {
        let now = now_ms();
        let window = self.rate_windows.entry(chat_id).or_default();
        while let Some(ts) = window.front().copied() {
            if now.saturating_sub(ts) > 60_000 {
                let _ = window.pop_front();
            } else {
                break;
            }
        }
        if window.len() >= self.config.max_messages_per_minute {
            return Err(KelvinError::Timeout(format!(
                "telegram chat {chat_id} exceeded max messages per minute"
            )));
        }
        window.push_back(now);
        Ok(())
    }

    fn enforce_pairing_policy(&mut self, chat_id: i64) -> KelvinErrorOr<Option<String>> {
        let allowlisted = self.config.allow_chat_ids.contains(&chat_id);
        let paired = self.paired_chat_ids.contains(&chat_id);

        if !self.config.pairing_enabled {
            if !self.config.allow_chat_ids.is_empty() && !allowlisted {
                return Err(KelvinError::NotFound(format!(
                    "telegram chat {chat_id} is not allowlisted"
                )));
            }
            return Ok(None);
        }

        if allowlisted || paired {
            return Ok(None);
        }

        let existing = self
            .pending_pair_codes
            .iter()
            .find_map(|(code, pending_chat)| {
                if *pending_chat == chat_id {
                    Some(code.clone())
                } else {
                    None
                }
            });
        if let Some(code) = existing {
            return Ok(Some(code));
        }

        let seed = now_ms() as i64 ^ chat_id;
        let numeric = seed.unsigned_abs() % 900_000 + 100_000;
        let code = format!("{numeric:06}");
        self.pending_pair_codes.insert(code.clone(), chat_id);
        Ok(Some(code))
    }

    async fn send_message_with_retry(&self, chat_id: i64, text: &str) -> KelvinErrorOr<()> {
        let Some(bot_token) = &self.config.bot_token else {
            return Ok(());
        };

        let endpoint = format!(
            "{}/bot{}/sendMessage",
            self.config.api_base_url.trim_end_matches('/'),
            bot_token
        );
        let body = json!({
            "chat_id": chat_id,
            "text": text,
        });

        let mut last_error = None;
        for attempt in 0..=self.config.outbound_max_retries {
            match self.client.post(&endpoint).json(&body).send().await {
                Ok(response) if response.status().is_success() => return Ok(()),
                Ok(response) => {
                    last_error = Some(format!(
                        "telegram send failed with status {}",
                        response.status()
                    ));
                }
                Err(_) => {
                    last_error = Some("telegram send transport error".to_string());
                }
            }
            if attempt < self.config.outbound_max_retries {
                sleep(Duration::from_millis(
                    self.config.outbound_retry_backoff_ms.max(1),
                ))
                .await;
            }
        }

        Err(KelvinError::Backend(format!(
            "telegram outbound delivery failed after retries: {}",
            last_error.unwrap_or_else(|| "unknown error".to_string())
        )))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TelegramPairApproveRequest {
    pub code: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telegram_allowlist_parsing_rejects_invalid_ids() {
        let err = parse_chat_id_set("1,abc,3").expect_err("invalid chat id should fail");
        assert!(matches!(err, KelvinError::InvalidInput(_)));
    }

    #[test]
    fn telegram_pairing_generates_stable_existing_code_per_pending_chat() {
        let config = TelegramChannelConfig {
            enabled: true,
            bot_token: None,
            api_base_url: "https://api.telegram.org".to_string(),
            allow_chat_ids: HashSet::new(),
            pairing_enabled: true,
            max_messages_per_minute: 10,
            max_seen_delivery_ids: 128,
            outbound_max_retries: 0,
            outbound_retry_backoff_ms: 1,
        };
        let mut adapter = TelegramChannelAdapter::new(config).expect("adapter");
        let code_one = adapter
            .enforce_pairing_policy(42)
            .expect("policy")
            .expect("code");
        let code_two = adapter
            .enforce_pairing_policy(42)
            .expect("policy")
            .expect("code");
        assert_eq!(code_one, code_two);
    }
}
