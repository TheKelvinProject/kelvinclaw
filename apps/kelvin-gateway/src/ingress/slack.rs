use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use ring::hmac;
use serde::Deserialize;
use serde_json::json;

use kelvin_core::now_ms;

use crate::channels::{ChannelKind, SlackIngressRequest};

use super::{
    channel_enabled, decode_hex, json_error, json_response, record_webhook_denied,
    record_webhook_verified, IngressAppState,
};

#[derive(Debug, Deserialize)]
struct SlackWebhookEnvelope {
    #[serde(rename = "type")]
    kind: String,
    challenge: Option<String>,
    event_id: Option<String>,
    event: Option<SlackEvent>,
}

#[derive(Debug, Deserialize)]
struct SlackEvent {
    #[serde(rename = "type")]
    kind: String,
    subtype: Option<String>,
    bot_id: Option<String>,
    channel: Option<String>,
    user: Option<String>,
    text: Option<String>,
}

pub(super) async fn handle(
    State(state): State<IngressAppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let kind = ChannelKind::Slack;
    let retry_hint = headers.contains_key("x-slack-retry-num");
    if !channel_enabled(&state.gateway, kind).await {
        return json_error(
            StatusCode::NOT_FOUND,
            "channel_disabled",
            "slack channel is not enabled",
        );
    }

    let Some(signing_secret) = state.config.slack.signing_secret.as_deref() else {
        let message = "slack signing secret is not configured";
        record_webhook_denied(
            &state.gateway,
            kind,
            StatusCode::SERVICE_UNAVAILABLE,
            retry_hint,
            message,
        )
        .await;
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "verification_unavailable",
            message,
        );
    };

    let timestamp = match header_str(&headers, "x-slack-request-timestamp") {
        Ok(value) => value,
        Err(()) => {
            record_webhook_denied(
                &state.gateway,
                kind,
                StatusCode::UNAUTHORIZED,
                retry_hint,
                "missing slack request timestamp",
            )
            .await;
            return json_error(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "missing x-slack-request-timestamp",
            );
        }
    };
    let signature = match header_str(&headers, "x-slack-signature") {
        Ok(value) => value,
        Err(()) => {
            record_webhook_denied(
                &state.gateway,
                kind,
                StatusCode::UNAUTHORIZED,
                retry_hint,
                "missing slack signature",
            )
            .await;
            return json_error(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "missing x-slack-signature",
            );
        }
    };

    let timestamp_secs = match timestamp.parse::<i64>() {
        Ok(value) => value,
        Err(_) => {
            let message = "invalid slack request timestamp";
            record_webhook_denied(
                &state.gateway,
                kind,
                StatusCode::BAD_REQUEST,
                retry_hint,
                message,
            )
            .await;
            return json_error(StatusCode::BAD_REQUEST, "invalid_payload", message);
        }
    };
    let now_secs = (now_ms() / 1_000) as i64;
    if now_secs.abs_diff(timestamp_secs) > state.config.slack.replay_window_secs {
        let message = "slack request timestamp is outside the replay window";
        record_webhook_denied(
            &state.gateway,
            kind,
            StatusCode::UNAUTHORIZED,
            retry_hint,
            message,
        )
        .await;
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized", message);
    }

    if let Err(message) = verify_signature(signing_secret, timestamp, signature, &body) {
        record_webhook_denied(
            &state.gateway,
            kind,
            StatusCode::UNAUTHORIZED,
            retry_hint,
            &message,
        )
        .await;
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized", &message);
    }

    let envelope = match serde_json::from_slice::<SlackWebhookEnvelope>(&body) {
        Ok(value) => value,
        Err(err) => {
            let message = format!("invalid slack webhook payload: {err}");
            record_webhook_denied(
                &state.gateway,
                kind,
                StatusCode::BAD_REQUEST,
                retry_hint,
                &message,
            )
            .await;
            return json_error(StatusCode::BAD_REQUEST, "invalid_payload", &message);
        }
    };

    match into_request(envelope) {
        SlackWebhookAction::Challenge(challenge) => {
            record_webhook_verified(&state.gateway, kind, StatusCode::OK, retry_hint).await;
            json_response(StatusCode::OK, json!({ "challenge": challenge }))
        }
        SlackWebhookAction::Ignore => {
            record_webhook_verified(&state.gateway, kind, StatusCode::OK, retry_hint).await;
            json_response(StatusCode::OK, json!({ "ok": true, "status": "ignored" }))
        }
        SlackWebhookAction::Accept(request) => {
            record_webhook_verified(&state.gateway, kind, StatusCode::OK, retry_hint).await;
            let runtime = state.gateway.runtime.clone();
            let channels = state.gateway.channels.clone();
            tokio::spawn(async move {
                let mut channels = channels.lock().await;
                if let Err(err) = channels.slack_ingest(&runtime, request).await {
                    eprintln!("slack webhook ingest failed: {err}");
                }
            });
            json_response(StatusCode::OK, json!({ "ok": true, "status": "accepted" }))
        }
        SlackWebhookAction::Deny(message) => {
            record_webhook_denied(
                &state.gateway,
                kind,
                StatusCode::BAD_REQUEST,
                retry_hint,
                &message,
            )
            .await;
            json_error(StatusCode::BAD_REQUEST, "invalid_payload", &message)
        }
    }
}

enum SlackWebhookAction {
    Challenge(String),
    Ignore,
    Accept(SlackIngressRequest),
    Deny(String),
}

fn into_request(envelope: SlackWebhookEnvelope) -> SlackWebhookAction {
    match envelope.kind.as_str() {
        "url_verification" => match envelope.challenge {
            Some(challenge) => SlackWebhookAction::Challenge(challenge),
            None => SlackWebhookAction::Deny(
                "slack url_verification request missing challenge".to_string(),
            ),
        },
        "event_callback" => {
            let Some(event) = envelope.event else {
                return SlackWebhookAction::Deny(
                    "slack event_callback request missing event object".to_string(),
                );
            };
            if event.kind != "message" || event.subtype.is_some() || event.bot_id.is_some() {
                return SlackWebhookAction::Ignore;
            }
            let Some(event_id) = envelope.event_id else {
                return SlackWebhookAction::Deny(
                    "slack event_callback request missing event_id".to_string(),
                );
            };
            let Some(channel_id) = event.channel else {
                return SlackWebhookAction::Deny("slack message event missing channel".to_string());
            };
            let Some(user_id) = event.user else {
                return SlackWebhookAction::Deny("slack message event missing user".to_string());
            };
            let Some(text) = event.text.map(|value| value.trim().to_string()) else {
                return SlackWebhookAction::Ignore;
            };
            if text.is_empty() {
                return SlackWebhookAction::Ignore;
            }
            SlackWebhookAction::Accept(SlackIngressRequest {
                delivery_id: format!("slack:{event_id}"),
                channel_id,
                user_id,
                text,
                timeout_ms: None,
                auth_token: None,
                session_id: None,
                workspace_dir: None,
            })
        }
        _ => SlackWebhookAction::Ignore,
    }
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, ()> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(())
}

fn verify_signature(
    signing_secret: &str,
    timestamp: &str,
    signature_header: &str,
    body: &[u8],
) -> Result<(), String> {
    let Some(encoded_signature) = signature_header.strip_prefix("v0=") else {
        return Err("slack signature must start with 'v0='".to_string());
    };
    let signature = decode_hex(encoded_signature)?;
    let key = hmac::Key::new(hmac::HMAC_SHA256, signing_secret.as_bytes());
    let mut payload = format!("v0:{timestamp}:").into_bytes();
    payload.extend_from_slice(body);
    hmac::verify(&key, &payload, &signature)
        .map_err(|_| "slack signature verification failed".to_string())
}
