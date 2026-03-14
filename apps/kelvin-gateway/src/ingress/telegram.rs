use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde::Deserialize;
use serde_json::json;

use crate::channels::{ChannelKind, TelegramIngressRequest};

use super::{
    channel_enabled, json_error, json_response, record_webhook_denied, record_webhook_verified,
    IngressAppState,
};

#[derive(Debug, Deserialize)]
struct TelegramUpdate {
    update_id: i64,
    message: Option<TelegramMessage>,
    edited_message: Option<TelegramMessage>,
    channel_post: Option<TelegramMessage>,
}

#[derive(Debug, Deserialize)]
struct TelegramMessage {
    chat: TelegramChat,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramChat {
    id: i64,
}

pub(super) async fn handle(
    State(state): State<IngressAppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let kind = ChannelKind::Telegram;
    if !channel_enabled(&state.gateway, kind).await {
        return json_error(
            StatusCode::NOT_FOUND,
            "channel_disabled",
            "telegram channel is not enabled",
        );
    }

    let Some(required_secret) = state.config.telegram.secret_token.as_deref() else {
        let message = "telegram webhook secret token is not configured";
        record_webhook_denied(
            &state.gateway,
            kind,
            StatusCode::SERVICE_UNAVAILABLE,
            false,
            message,
        )
        .await;
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "verification_unavailable",
            message,
        );
    };

    let provided_secret = headers
        .get("x-telegram-bot-api-secret-token")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if provided_secret != Some(required_secret) {
        let message = "telegram webhook secret token mismatch";
        record_webhook_denied(
            &state.gateway,
            kind,
            StatusCode::UNAUTHORIZED,
            false,
            message,
        )
        .await;
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized", message);
    }

    let update = match serde_json::from_slice::<TelegramUpdate>(&body) {
        Ok(value) => value,
        Err(err) => {
            let message = format!("invalid telegram webhook payload: {err}");
            record_webhook_denied(
                &state.gateway,
                kind,
                StatusCode::BAD_REQUEST,
                false,
                &message,
            )
            .await;
            return json_error(StatusCode::BAD_REQUEST, "invalid_payload", &message);
        }
    };

    let Some(request) = into_request(update) else {
        record_webhook_verified(&state.gateway, kind, StatusCode::OK, false).await;
        return json_response(StatusCode::OK, json!({ "ok": true, "status": "ignored" }));
    };

    record_webhook_verified(&state.gateway, kind, StatusCode::OK, false).await;
    let runtime = state.gateway.runtime.clone();
    let channels = state.gateway.channels.clone();
    tokio::spawn(async move {
        let mut channels = channels.lock().await;
        if let Err(err) = channels.telegram_ingest(&runtime, request).await {
            eprintln!("telegram webhook ingest failed: {err}");
        }
    });

    json_response(StatusCode::OK, json!({ "ok": true, "status": "accepted" }))
}

fn into_request(update: TelegramUpdate) -> Option<TelegramIngressRequest> {
    let message = update
        .message
        .or(update.edited_message)
        .or(update.channel_post)?;
    let text = message.text?.trim().to_string();
    if text.is_empty() {
        return None;
    }
    Some(TelegramIngressRequest {
        delivery_id: format!("telegram:{}", update.update_id),
        chat_id: message.chat.id,
        text,
        timeout_ms: None,
        auth_token: None,
        session_id: None,
        workspace_dir: None,
    })
}
