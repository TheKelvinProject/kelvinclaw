use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use ed25519_dalek::{Signer, SigningKey};
use futures_util::{SinkExt, StreamExt};
use kelvin_gateway::{
    run_gateway_with_listener_secure_and_ingress, GatewayIngressConfig, GatewaySecurityConfig,
};
use kelvin_sdk::{
    KelvinCliMemoryMode, KelvinSdkModelSelection, KelvinSdkRuntime, KelvinSdkRuntimeConfig,
};
use reqwest::Client;
use ring::hmac;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::{sleep, Duration};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct EnvVarRestore {
    key: &'static str,
    previous: Option<String>,
}

impl EnvVarRestore {
    fn set(key: &'static str, value: Option<&str>) -> Self {
        let previous = std::env::var(key).ok();
        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        Self { key, previous }
    }
}

impl Drop for EnvVarRestore {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

fn unique_workspace(prefix: &str) -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let path = std::env::temp_dir().join(format!("kelvin-http-ingress-test-{prefix}-{millis}"));
    std::fs::create_dir_all(&path).expect("create workspace");
    path
}

async fn start_gateway_with_ingress(
    auth_token: Option<&str>,
    ingress: GatewayIngressConfig,
) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener address");
    let runtime = KelvinSdkRuntime::initialize(KelvinSdkRuntimeConfig {
        workspace_dir: unique_workspace("runtime"),
        default_session_id: "main".to_string(),
        memory_mode: KelvinCliMemoryMode::Fallback,
        default_timeout_ms: 3_000,
        default_system_prompt: None,
        core_version: "0.1.0".to_string(),
        plugin_security_policy: Default::default(),
        load_installed_plugins: false,
        model_provider: KelvinSdkModelSelection::Echo,
        require_cli_plugin_tool: false,
        emit_stdout_events: false,
        state_dir: None,
        persist_runs: true,
        max_session_history_messages: 128,
        compact_to_messages: 64,
    })
    .await
    .expect("initialize runtime");

    let token = auth_token.map(|value| value.to_string());
    let handle = tokio::spawn(async move {
        let _ = run_gateway_with_listener_secure_and_ingress(
            listener,
            runtime,
            token,
            GatewaySecurityConfig::default(),
            ingress,
        )
        .await;
    });
    sleep(Duration::from_millis(75)).await;
    (format!("ws://{addr}"), handle)
}

async fn connect_gateway(
    url: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let (mut socket, _) = connect_async(url).await.expect("connect");
    send_request(
        &mut socket,
        "connect",
        "connect",
        json!({
            "auth": {"token": "secret"},
            "client_id": "gateway-http-ingress-test",
        }),
    )
    .await;
    let response = read_until_response(&mut socket, "connect").await;
    assert_eq!(response["ok"], json!(true));
    socket
}

async fn send_request(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    id: &str,
    method: &str,
    params: Value,
) {
    socket
        .send(Message::Text(
            json!({
                "type": "req",
                "id": id,
                "method": method,
                "params": params,
            })
            .to_string(),
        ))
        .await
        .expect("send request");
}

async fn read_until_response(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    target_id: &str,
) -> Value {
    loop {
        let message = socket.next().await.expect("frame").expect("message");
        let Message::Text(text) = message else {
            continue;
        };
        let frame: Value = serde_json::from_str(&text).expect("json frame");
        if frame.get("type") == Some(&Value::String("res".to_string()))
            && frame.get("id") == Some(&Value::String(target_id.to_string()))
        {
            return frame;
        }
    }
}

async fn ingress_base_url(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> String {
    let ingress = ingress_status(socket).await;
    format!(
        "http://{}{}",
        ingress["bind_addr"].as_str().expect("ingress bind addr"),
        ingress["base_path"].as_str().expect("ingress base path")
    )
}

async fn ingress_status(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Value {
    send_request(socket, "health-ingress", "health", json!({})).await;
    let response = read_until_response(socket, "health-ingress").await;
    assert_eq!(response["ok"], json!(true));
    assert_eq!(response["payload"]["ingress"]["enabled"], json!(true));
    response["payload"]["ingress"].clone()
}

async fn wait_for_channel_status<F>(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    method: &str,
    predicate: F,
) -> Value
where
    F: Fn(&Value) -> bool,
{
    for attempt in 0..40 {
        let request_id = format!("status-{method}-{attempt}");
        send_request(socket, &request_id, method, json!({})).await;
        let response = read_until_response(socket, &request_id).await;
        assert_eq!(response["ok"], json!(true));
        if predicate(&response["payload"]) {
            return response["payload"].clone();
        }
        sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {method} status predicate");
}

#[tokio::test]
async fn telegram_http_webhook_ingests_and_updates_health() {
    let _guard = ENV_LOCK.lock().await;
    let _env_restore = [
        EnvVarRestore::set("KELVIN_TELEGRAM_ENABLED", Some("true")),
        EnvVarRestore::set("KELVIN_TELEGRAM_PAIRING_ENABLED", Some("false")),
        EnvVarRestore::set("KELVIN_TELEGRAM_BOT_TOKEN", None),
        EnvVarRestore::set(
            "KELVIN_TELEGRAM_WEBHOOK_SECRET_TOKEN",
            Some("telegram-secret"),
        ),
    ];
    let ingress = GatewayIngressConfig::from_env_overrides(
        Some("127.0.0.1:0".parse().expect("ingress bind")),
        None,
        None,
        false,
    )
    .expect("ingress config");
    let (ws_url, server_handle) = start_gateway_with_ingress(Some("secret"), ingress).await;
    let mut socket = connect_gateway(&ws_url).await;
    let ingress_base = ingress_base_url(&mut socket).await;

    let response = Client::new()
        .post(format!("{ingress_base}/telegram"))
        .header("X-Telegram-Bot-Api-Secret-Token", "telegram-secret")
        .json(&json!({
            "update_id": 1001,
            "message": {
                "chat": {"id": 42},
                "text": "hello from telegram webhook"
            }
        }))
        .send()
        .await
        .expect("telegram webhook");
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let status = wait_for_channel_status(&mut socket, "channel.telegram.status", |payload| {
        payload["metrics"]["webhook_accepted_total"]
            .as_u64()
            .unwrap_or_default()
            >= 1
            && payload["metrics"]["ingest_total"]
                .as_u64()
                .unwrap_or_default()
                >= 1
    })
    .await;
    assert_eq!(
        status["ingress_verification"]["method"],
        json!("telegram_secret_token")
    );
    assert_eq!(status["ingress_verification"]["configured"], json!(true));
    assert_eq!(
        status["ingress_connectivity"]["last_status_code"],
        json!(200)
    );
    assert_eq!(status["metrics"]["verification_failed_total"], json!(0));

    server_handle.abort();
}

#[tokio::test]
async fn slack_http_webhook_verifies_signatures_and_tracks_retries() {
    let _guard = ENV_LOCK.lock().await;
    let _env_restore = [
        EnvVarRestore::set("KELVIN_SLACK_ENABLED", Some("true")),
        EnvVarRestore::set("KELVIN_SLACK_BOT_TOKEN", None),
        EnvVarRestore::set("KELVIN_SLACK_SIGNING_SECRET", Some("slack-signing-secret")),
    ];
    let ingress = GatewayIngressConfig::from_env_overrides(
        Some("127.0.0.1:0".parse().expect("ingress bind")),
        None,
        None,
        false,
    )
    .expect("ingress config");
    let (ws_url, server_handle) = start_gateway_with_ingress(Some("secret"), ingress).await;
    let mut socket = connect_gateway(&ws_url).await;
    let ingress_base = ingress_base_url(&mut socket).await;
    let client = Client::new();

    let challenge_body = json!({
        "type": "url_verification",
        "challenge": "challenge-token"
    })
    .to_string();
    let challenge_response = post_signed_slack(
        &client,
        &format!("{ingress_base}/slack"),
        "slack-signing-secret",
        &challenge_body,
        None,
    )
    .await;
    assert_eq!(challenge_response.status(), reqwest::StatusCode::OK);
    let challenge_payload: Value = challenge_response.json().await.expect("challenge payload");
    assert_eq!(challenge_payload["challenge"], json!("challenge-token"));

    let event_body = json!({
        "type": "event_callback",
        "event_id": "Ev123",
        "event": {
            "type": "message",
            "channel": "C1",
            "user": "U1",
            "text": "hello from slack webhook"
        }
    })
    .to_string();
    let event_response = post_signed_slack(
        &client,
        &format!("{ingress_base}/slack"),
        "slack-signing-secret",
        &event_body,
        Some("1"),
    )
    .await;
    assert_eq!(event_response.status(), reqwest::StatusCode::OK);

    let invalid_response = client
        .post(format!("{ingress_base}/slack"))
        .header("X-Slack-Request-Timestamp", slack_timestamp())
        .header("X-Slack-Signature", "v0=deadbeef")
        .header("Content-Type", "application/json")
        .body(event_body.clone())
        .send()
        .await
        .expect("invalid slack request");
    assert_eq!(invalid_response.status(), reqwest::StatusCode::UNAUTHORIZED);

    let status = wait_for_channel_status(&mut socket, "channel.slack.status", |payload| {
        payload["metrics"]["webhook_retry_total"]
            .as_u64()
            .unwrap_or_default()
            >= 1
            && payload["metrics"]["verification_failed_total"]
                .as_u64()
                .unwrap_or_default()
                >= 1
            && payload["metrics"]["ingest_total"]
                .as_u64()
                .unwrap_or_default()
                >= 1
    })
    .await;
    assert_eq!(
        status["ingress_verification"]["method"],
        json!("slack_signing_secret")
    );
    assert_eq!(status["ingress_verification"]["configured"], json!(true));
    assert!(
        status["metrics"]["webhook_accepted_total"]
            .as_u64()
            .unwrap_or_default()
            >= 2
    );
    assert!(
        status["metrics"]["webhook_denied_total"]
            .as_u64()
            .unwrap_or_default()
            >= 1
    );

    server_handle.abort();
}

#[tokio::test]
async fn discord_http_interactions_verify_signatures_and_dispatch() {
    let _guard = ENV_LOCK.lock().await;
    let signing_key = SigningKey::from_bytes(&[7_u8; 32]);
    let public_key_hex = hex_encode(&signing_key.verifying_key().to_bytes());
    let _env_restore = [
        EnvVarRestore::set("KELVIN_DISCORD_ENABLED", Some("true")),
        EnvVarRestore::set("KELVIN_DISCORD_BOT_TOKEN", None),
        EnvVarRestore::set(
            "KELVIN_DISCORD_INTERACTIONS_PUBLIC_KEY",
            Some(&public_key_hex),
        ),
    ];
    let ingress = GatewayIngressConfig::from_env_overrides(
        Some("127.0.0.1:0".parse().expect("ingress bind")),
        None,
        None,
        false,
    )
    .expect("ingress config");
    let (ws_url, server_handle) = start_gateway_with_ingress(Some("secret"), ingress).await;
    let mut socket = connect_gateway(&ws_url).await;
    let ingress_base = ingress_base_url(&mut socket).await;
    let client = Client::new();

    let ping_body = json!({
        "id": "discord-ping-1",
        "type": 1
    })
    .to_string();
    let ping_response = post_signed_discord(
        &client,
        &format!("{ingress_base}/discord"),
        &signing_key,
        &ping_body,
    )
    .await;
    assert_eq!(ping_response.status(), reqwest::StatusCode::OK);
    let ping_payload: Value = ping_response.json().await.expect("ping payload");
    assert_eq!(ping_payload["type"], json!(1));

    let command_body = json!({
        "id": "discord-cmd-1",
        "type": 2,
        "channel_id": "D1",
        "member": {"user": {"id": "U1"}},
        "data": {
            "name": "ask",
            "options": [{"name": "prompt", "value": "hello from discord webhook"}]
        }
    })
    .to_string();
    let command_response = post_signed_discord(
        &client,
        &format!("{ingress_base}/discord"),
        &signing_key,
        &command_body,
    )
    .await;
    assert_eq!(command_response.status(), reqwest::StatusCode::OK);
    let command_payload: Value = command_response.json().await.expect("command payload");
    assert_eq!(command_payload["type"], json!(4));

    let status = wait_for_channel_status(&mut socket, "channel.discord.status", |payload| {
        payload["metrics"]["webhook_accepted_total"]
            .as_u64()
            .unwrap_or_default()
            >= 2
            && payload["metrics"]["ingest_total"]
                .as_u64()
                .unwrap_or_default()
                >= 1
    })
    .await;
    assert_eq!(
        status["ingress_verification"]["method"],
        json!("discord_ed25519")
    );
    assert_eq!(status["ingress_verification"]["configured"], json!(true));
    assert_eq!(
        status["ingress_connectivity"]["last_status_code"],
        json!(200)
    );

    server_handle.abort();
}

#[tokio::test]
async fn ingress_listener_serves_operator_console_assets() {
    let _guard = ENV_LOCK.lock().await;
    let ingress = GatewayIngressConfig::from_env_overrides(
        Some("127.0.0.1:0".parse().expect("ingress bind")),
        None,
        None,
        false,
    )
    .expect("ingress config");
    let (ws_url, server_handle) = start_gateway_with_ingress(Some("secret"), ingress).await;
    let mut socket = connect_gateway(&ws_url).await;
    let ingress = ingress_status(&mut socket).await;
    let root = format!(
        "http://{}",
        ingress["bind_addr"].as_str().expect("ingress bind addr")
    );
    assert_eq!(ingress["operator_ui_path"], json!("/operator/"));

    let index = Client::new()
        .get(format!("{root}/operator/"))
        .send()
        .await
        .expect("operator index");
    assert_eq!(index.status(), reqwest::StatusCode::OK);
    let body = index.text().await.expect("operator index body");
    assert!(body.contains("KelvinClaw Operator"));

    let script = Client::new()
        .get(format!("{root}/operator/app.js"))
        .send()
        .await
        .expect("operator script");
    assert_eq!(script.status(), reqwest::StatusCode::OK);

    server_handle.abort();
}

async fn post_signed_slack(
    client: &Client,
    url: &str,
    signing_secret: &str,
    body: &str,
    retry_num: Option<&str>,
) -> reqwest::Response {
    let timestamp = slack_timestamp();
    let key = hmac::Key::new(hmac::HMAC_SHA256, signing_secret.as_bytes());
    let payload = format!("v0:{timestamp}:{body}");
    let signature = hmac::sign(&key, payload.as_bytes());
    let mut request = client
        .post(url)
        .header("X-Slack-Request-Timestamp", &timestamp)
        .header(
            "X-Slack-Signature",
            format!("v0={}", hex_encode(signature.as_ref())),
        )
        .header("Content-Type", "application/json")
        .body(body.to_string());
    if let Some(retry_num) = retry_num {
        request = request.header("X-Slack-Retry-Num", retry_num);
    }
    request.send().await.expect("signed slack request")
}

fn slack_timestamp() -> String {
    (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default())
    .to_string()
}

async fn post_signed_discord(
    client: &Client,
    url: &str,
    signing_key: &SigningKey,
    body: &str,
) -> reqwest::Response {
    let timestamp = slack_timestamp();
    let mut payload = timestamp.as_bytes().to_vec();
    payload.extend_from_slice(body.as_bytes());
    let signature = signing_key.sign(&payload);
    client
        .post(url)
        .header("X-Signature-Timestamp", &timestamp)
        .header(
            "X-Signature-Ed25519",
            hex_encode(signature.to_bytes().as_ref()),
        )
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .expect("signed discord request")
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}
