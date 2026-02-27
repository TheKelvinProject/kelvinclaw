use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use kelvin_gateway::run_gateway_with_listener;
use kelvin_sdk::{
    KelvinCliMemoryMode, KelvinSdkModelSelection, KelvinSdkRuntime, KelvinSdkRuntimeConfig,
};
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
    let path = std::env::temp_dir().join(format!("kelvin-channel-conformance-{prefix}-{millis}"));
    std::fs::create_dir_all(&path).expect("create workspace");
    path
}

async fn start_gateway(auth_token: Option<&str>) -> (String, JoinHandle<()>) {
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
        let _ = run_gateway_with_listener(listener, runtime, token).await;
    });
    sleep(Duration::from_millis(75)).await;
    (format!("ws://{addr}"), handle)
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

#[tokio::test]
async fn conformance_delivery_ordering_and_idempotency() {
    let _guard = ENV_LOCK.lock().await;
    let _env_restore = [
        EnvVarRestore::set("KELVIN_SLACK_ENABLED", Some("true")),
        EnvVarRestore::set("KELVIN_SLACK_BOT_TOKEN", None),
        EnvVarRestore::set("KELVIN_SLACK_MAX_MESSAGES_PER_MINUTE", Some("100")),
    ];

    let (url, server_handle) = start_gateway(Some("secret")).await;
    let (mut socket, _) = connect_async(url).await.expect("connect");

    send_request(
        &mut socket,
        "connect",
        "connect",
        json!({
            "auth": {"token": "secret"},
            "client_id": "channel-conformance"
        }),
    )
    .await;
    assert_eq!(
        read_until_response(&mut socket, "connect").await["ok"],
        json!(true)
    );

    send_request(
        &mut socket,
        "msg-1",
        "channel.slack.ingest",
        json!({
            "delivery_id": "delivery-1",
            "channel_id": "C-ORDER",
            "user_id": "U-ORDER",
            "text": "first"
        }),
    )
    .await;
    let first = read_until_response(&mut socket, "msg-1").await;
    assert_eq!(first["ok"], json!(true));
    assert_eq!(first["payload"]["status"], json!("completed"));
    assert!(first["payload"]["response_text"]
        .as_str()
        .unwrap_or_default()
        .contains("first"));

    send_request(
        &mut socket,
        "msg-2",
        "channel.slack.ingest",
        json!({
            "delivery_id": "delivery-2",
            "channel_id": "C-ORDER",
            "user_id": "U-ORDER",
            "text": "second"
        }),
    )
    .await;
    let second = read_until_response(&mut socket, "msg-2").await;
    assert_eq!(second["ok"], json!(true));
    assert_eq!(second["payload"]["status"], json!("completed"));
    assert!(second["payload"]["response_text"]
        .as_str()
        .unwrap_or_default()
        .contains("second"));

    send_request(
        &mut socket,
        "msg-2-dup",
        "channel.slack.ingest",
        json!({
            "delivery_id": "delivery-2",
            "channel_id": "C-ORDER",
            "user_id": "U-ORDER",
            "text": "second"
        }),
    )
    .await;
    let dup = read_until_response(&mut socket, "msg-2-dup").await;
    assert_eq!(dup["ok"], json!(true));
    assert_eq!(dup["payload"]["status"], json!("deduped"));

    server_handle.abort();
}

#[tokio::test]
async fn conformance_auth_mismatch_is_rejected() {
    let _guard = ENV_LOCK.lock().await;
    let _env_restore = [
        EnvVarRestore::set("KELVIN_SLACK_ENABLED", Some("true")),
        EnvVarRestore::set("KELVIN_SLACK_INGRESS_TOKEN", Some("expected-token")),
        EnvVarRestore::set("KELVIN_SLACK_BOT_TOKEN", None),
    ];

    let (url, server_handle) = start_gateway(Some("secret")).await;
    let (mut socket, _) = connect_async(url).await.expect("connect");

    send_request(
        &mut socket,
        "connect-auth",
        "connect",
        json!({
            "auth": {"token": "secret"},
            "client_id": "channel-conformance"
        }),
    )
    .await;
    assert_eq!(
        read_until_response(&mut socket, "connect-auth").await["ok"],
        json!(true)
    );

    send_request(
        &mut socket,
        "auth-mismatch",
        "channel.slack.ingest",
        json!({
            "delivery_id": "delivery-auth",
            "channel_id": "C-AUTH",
            "user_id": "U-AUTH",
            "text": "hello",
            "auth_token": "wrong-token"
        }),
    )
    .await;
    let response = read_until_response(&mut socket, "auth-mismatch").await;
    assert_eq!(response["ok"], json!(false));
    assert_eq!(response["error"]["code"], json!("not_found"));

    server_handle.abort();
}

#[tokio::test]
async fn conformance_flood_handling_is_enforced() {
    let _guard = ENV_LOCK.lock().await;
    let _env_restore = [
        EnvVarRestore::set("KELVIN_DISCORD_ENABLED", Some("true")),
        EnvVarRestore::set("KELVIN_DISCORD_BOT_TOKEN", None),
        EnvVarRestore::set("KELVIN_DISCORD_MAX_MESSAGES_PER_MINUTE", Some("1")),
    ];

    let (url, server_handle) = start_gateway(Some("secret")).await;
    let (mut socket, _) = connect_async(url).await.expect("connect");

    send_request(
        &mut socket,
        "connect-flood",
        "connect",
        json!({
            "auth": {"token": "secret"},
            "client_id": "channel-conformance"
        }),
    )
    .await;
    assert_eq!(
        read_until_response(&mut socket, "connect-flood").await["ok"],
        json!(true)
    );

    send_request(
        &mut socket,
        "flood-1",
        "channel.discord.ingest",
        json!({
            "delivery_id": "flood-1",
            "channel_id": "D-FLOOD",
            "user_id": "U-FLOOD",
            "text": "first"
        }),
    )
    .await;
    let first = read_until_response(&mut socket, "flood-1").await;
    assert_eq!(first["ok"], json!(true));

    send_request(
        &mut socket,
        "flood-2",
        "channel.discord.ingest",
        json!({
            "delivery_id": "flood-2",
            "channel_id": "D-FLOOD",
            "user_id": "U-FLOOD",
            "text": "second"
        }),
    )
    .await;
    let second = read_until_response(&mut socket, "flood-2").await;
    assert_eq!(second["ok"], json!(false));
    assert_eq!(second["error"]["code"], json!("timeout"));

    server_handle.abort();
}
