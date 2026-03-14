use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use kelvin_gateway::{
    run_gateway_with_listener_secure, GatewaySecurityConfig, GATEWAY_METHODS_V1,
    GATEWAY_PROTOCOL_VERSION,
};
use kelvin_sdk::{
    KelvinCliMemoryMode, KelvinSdkModelSelection, KelvinSdkRuntime, KelvinSdkRuntimeConfig,
    NewScheduledTask, ScheduleReplyTarget,
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
    let path = std::env::temp_dir().join(format!("kelvin-gateway-test-{prefix}-{millis}"));
    std::fs::create_dir_all(&path).expect("create workspace");
    path
}

async fn start_gateway(auth_token: Option<&str>) -> (String, JoinHandle<()>) {
    start_gateway_with_security(auth_token, GatewaySecurityConfig::default()).await
}

async fn start_gateway_with_security(
    auth_token: Option<&str>,
    security: GatewaySecurityConfig,
) -> (String, JoinHandle<()>) {
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
    start_gateway_with_runtime(runtime, auth_token, security).await
}

async fn start_gateway_with_runtime(
    runtime: KelvinSdkRuntime,
    auth_token: Option<&str>,
    security: GatewaySecurityConfig,
) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener address");

    let token = auth_token.map(|value| value.to_string());
    let handle = tokio::spawn(async move {
        let _ = run_gateway_with_listener_secure(listener, runtime, token, security).await;
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
async fn gateway_rejects_non_connect_first_frame() {
    let _guard = ENV_LOCK.lock().await;
    let (url, server_handle) = start_gateway(None).await;
    let (mut socket, _) = connect_async(url).await.expect("connect");

    send_request(&mut socket, "req-1", "health", json!({})).await;
    let response = read_until_response(&mut socket, "req-1").await;
    assert_eq!(response["ok"], json!(false));
    assert_eq!(response["error"]["code"], json!("handshake_required"));

    server_handle.abort();
}

#[tokio::test]
async fn gateway_enforces_auth_token_on_connect() {
    let _guard = ENV_LOCK.lock().await;
    let (url, server_handle) = start_gateway(Some("secret")).await;
    let (mut socket, _) = connect_async(url).await.expect("connect");

    send_request(&mut socket, "connect-1", "connect", json!({})).await;
    let response = read_until_response(&mut socket, "connect-1").await;
    assert_eq!(response["ok"], json!(false));
    assert_eq!(response["error"]["code"], json!("unauthorized"));

    server_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_unknown_method_with_method_not_found() {
    let _guard = ENV_LOCK.lock().await;
    let (url, server_handle) = start_gateway(Some("secret")).await;
    let (mut socket, _) = connect_async(url).await.expect("connect");

    send_request(
        &mut socket,
        "connect-unknown",
        "connect",
        json!({
            "auth": {"token": "secret"},
            "client_id": "integration-test",
        }),
    )
    .await;
    let connect_response = read_until_response(&mut socket, "connect-unknown").await;
    assert_eq!(connect_response["ok"], json!(true));

    send_request(
        &mut socket,
        "unknown-1",
        "channel.unknown.dispatch",
        json!({}),
    )
    .await;
    let unknown_response = read_until_response(&mut socket, "unknown-1").await;
    assert_eq!(unknown_response["ok"], json!(false));
    assert_eq!(unknown_response["error"]["code"], json!("method_not_found"));

    server_handle.abort();
}

#[tokio::test]
async fn gateway_agent_submit_wait_and_idempotency_flow_works() {
    let _guard = ENV_LOCK.lock().await;
    let (url, server_handle) = start_gateway(Some("secret")).await;
    let (mut socket, _) = connect_async(url).await.expect("connect");

    send_request(
        &mut socket,
        "connect-ok",
        "connect",
        json!({
            "auth": {"token": "secret"},
            "client_id": "integration-test",
        }),
    )
    .await;
    let connect_response = read_until_response(&mut socket, "connect-ok").await;
    assert_eq!(connect_response["ok"], json!(true));
    assert_eq!(
        connect_response["payload"]["protocol_version"],
        json!(GATEWAY_PROTOCOL_VERSION)
    );
    assert_eq!(
        connect_response["payload"]["supported_methods"],
        json!(GATEWAY_METHODS_V1)
    );

    send_request(&mut socket, "health-1", "health", json!({})).await;
    let health_response = read_until_response(&mut socket, "health-1").await;
    assert_eq!(health_response["ok"], json!(true));
    assert_eq!(
        health_response["payload"]["protocol_version"],
        json!(GATEWAY_PROTOCOL_VERSION)
    );
    assert_eq!(
        health_response["payload"]["supported_methods"],
        json!(GATEWAY_METHODS_V1)
    );
    assert_eq!(
        health_response["payload"]["security"]["transport"],
        json!("ws")
    );
    assert_eq!(
        health_response["payload"]["security"]["bind_scope"],
        json!("loopback")
    );
    assert_eq!(
        health_response["payload"]["security"]["max_inflight_requests_per_connection"],
        json!(1)
    );

    send_request(
        &mut socket,
        "agent-1",
        "agent",
        json!({
            "request_id": "abc-123",
            "prompt": "Hello from gateway test",
            "session_id": "session-test",
            "timeout_ms": 2000,
        }),
    )
    .await;
    let submit_first = read_until_response(&mut socket, "agent-1").await;
    assert_eq!(submit_first["ok"], json!(true));
    let run_id = submit_first["payload"]["run_id"]
        .as_str()
        .expect("run id")
        .to_string();
    assert_eq!(submit_first["payload"]["deduped"], json!(false));

    send_request(
        &mut socket,
        "agent-1-dup",
        "agent",
        json!({
            "request_id": "abc-123",
            "prompt": "Hello from gateway test",
            "session_id": "session-test",
            "timeout_ms": 2000,
        }),
    )
    .await;
    let submit_second = read_until_response(&mut socket, "agent-1-dup").await;
    assert_eq!(submit_second["ok"], json!(true));
    assert_eq!(submit_second["payload"]["run_id"], json!(run_id));
    assert_eq!(submit_second["payload"]["deduped"], json!(true));

    send_request(
        &mut socket,
        "wait-1",
        "agent.wait",
        json!({
            "run_id": run_id,
            "timeout_ms": 5000,
        }),
    )
    .await;
    let wait_response = read_until_response(&mut socket, "wait-1").await;
    assert_eq!(wait_response["ok"], json!(true));
    assert_eq!(wait_response["payload"]["status"], json!("ok"));

    server_handle.abort();
}

#[tokio::test]
async fn gateway_exposes_scheduler_list_and_history() {
    let _guard = ENV_LOCK.lock().await;
    let _env_restore = [
        EnvVarRestore::set("KELVIN_SLACK_ENABLED", Some("true")),
        EnvVarRestore::set("KELVIN_SLACK_BOT_TOKEN", None),
    ];

    let workspace = unique_workspace("scheduler-runtime");
    let runtime = KelvinSdkRuntime::initialize(KelvinSdkRuntimeConfig {
        workspace_dir: workspace.clone(),
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
        state_dir: Some(workspace.join(".kelvin/state")),
        persist_runs: true,
        max_session_history_messages: 128,
        compact_to_messages: 64,
    })
    .await
    .expect("initialize runtime");
    runtime
        .scheduler_store()
        .add_schedule(NewScheduledTask {
            id: "schedule-api".to_string(),
            cron: "* * * * *".to_string(),
            prompt: "hello from schedule".to_string(),
            session_id: Some("schedule-session".to_string()),
            workspace_dir: Some(workspace.to_string_lossy().to_string()),
            timeout_ms: Some(2_000),
            system_prompt: None,
            memory_query: None,
            reply_target: Some(ScheduleReplyTarget {
                channel: "slack".to_string(),
                account_id: "C-SCHEDULE".to_string(),
            }),
            created_by_session: "seed-session".to_string(),
            created_at_ms: 0,
            approval_reason: "test schedule".to_string(),
        })
        .expect("seed schedule");

    let (url, server_handle) =
        start_gateway_with_runtime(runtime, Some("secret"), GatewaySecurityConfig::default()).await;
    let (mut socket, _) = connect_async(url).await.expect("connect");

    send_request(
        &mut socket,
        "connect-scheduler",
        "connect",
        json!({
            "auth": {"token": "secret"},
            "client_id": "scheduler-test",
        }),
    )
    .await;
    assert_eq!(
        read_until_response(&mut socket, "connect-scheduler").await["ok"],
        json!(true)
    );

    send_request(&mut socket, "schedule-list", "schedule.list", json!({})).await;
    let list = read_until_response(&mut socket, "schedule-list").await;
    assert_eq!(list["ok"], json!(true));
    assert_eq!(list["payload"]["status"]["schedule_count"], json!(1));
    assert_eq!(list["payload"]["schedules"][0]["id"], json!("schedule-api"));

    let mut history = json!({});
    for _ in 0..12 {
        send_request(
            &mut socket,
            "schedule-history",
            "schedule.history",
            json!({
                "schedule_id": "schedule-api",
                "limit": 10,
            }),
        )
        .await;
        history = read_until_response(&mut socket, "schedule-history").await;
        let completed = history["payload"]["slots"]
            .as_array()
            .map(|slots| slots.iter().any(|slot| slot["phase"] == json!("completed")))
            .unwrap_or(false);
        if completed {
            break;
        }
        sleep(Duration::from_millis(250)).await;
    }

    assert_eq!(history["ok"], json!(true));
    assert!(history["payload"]["slots"]
        .as_array()
        .map(|slots| slots.iter().any(|slot| slot["phase"] == json!("completed")))
        .unwrap_or(false));
    assert!(history["payload"]["audit"]
        .as_array()
        .map(|entries| entries
            .iter()
            .any(|entry| entry["kind"] == json!("slot_completed")))
        .unwrap_or(false));

    send_request(&mut socket, "health-scheduler", "health", json!({})).await;
    let health = read_until_response(&mut socket, "health-scheduler").await;
    assert_eq!(health["ok"], json!(true));
    assert_eq!(
        health["payload"]["scheduler"]["status"]["schedule_count"],
        json!(1)
    );
    assert!(
        health["payload"]["scheduler"]["metrics"]["claimed_total"]
            .as_u64()
            .unwrap_or_default()
            >= 1
    );

    server_handle.abort();
}

#[tokio::test]
async fn gateway_applies_auth_backoff_after_failed_connect_attempts() {
    let _guard = ENV_LOCK.lock().await;
    let security = GatewaySecurityConfig {
        auth_failure_threshold: 1,
        auth_failure_backoff_ms: 5_000,
        ..GatewaySecurityConfig::default()
    };
    let (url, server_handle) = start_gateway_with_security(Some("secret"), security).await;

    let (mut first_socket, _) = connect_async(url.clone()).await.expect("connect");
    send_request(&mut first_socket, "connect-fail-1", "connect", json!({})).await;
    let first_response = read_until_response(&mut first_socket, "connect-fail-1").await;
    assert_eq!(first_response["ok"], json!(false));
    assert_eq!(first_response["error"]["code"], json!("unauthorized"));

    let (mut second_socket, _) = connect_async(url).await.expect("connect");
    let second_response = read_until_response(&mut second_socket, "").await;
    assert_eq!(second_response["ok"], json!(false));
    assert_eq!(second_response["error"]["code"], json!("unauthorized"));
    assert!(
        second_response["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("backoff"),
        "expected backoff message in {:?}",
        second_response
    );

    server_handle.abort();
}

#[tokio::test]
async fn gateway_closes_connection_on_oversized_frame() {
    let _guard = ENV_LOCK.lock().await;
    let security = GatewaySecurityConfig {
        max_message_size_bytes: 1024,
        max_frame_size_bytes: 512,
        ..GatewaySecurityConfig::default()
    };
    let (url, server_handle) = start_gateway_with_security(Some("secret"), security).await;
    let (mut socket, _) = connect_async(url).await.expect("connect");

    send_request(
        &mut socket,
        "connect-small",
        "connect",
        json!({
            "auth": {"token": "secret"},
            "client_id": "integration-test",
        }),
    )
    .await;
    let connect_response = read_until_response(&mut socket, "connect-small").await;
    assert_eq!(connect_response["ok"], json!(true));

    let oversized = format!("{{\"type\":\"req\",\"id\":\"huge\",\"method\":\"health\",\"params\":{{\"padding\":\"{}\"}}}}", "x".repeat(512));
    socket
        .send(Message::Text(oversized))
        .await
        .expect("send oversized frame");

    match socket.next().await {
        Some(Ok(Message::Close(_))) | None => {}
        Some(Err(_)) => {}
        other => panic!("expected socket close or error, got {other:?}"),
    }

    server_handle.abort();
}

#[tokio::test]
async fn gateway_telegram_channel_pairing_and_dispatch_flow_works() {
    let _guard = ENV_LOCK.lock().await;
    let _env_restore = [
        EnvVarRestore::set("KELVIN_TELEGRAM_ENABLED", Some("true")),
        EnvVarRestore::set("KELVIN_TELEGRAM_PAIRING_ENABLED", Some("true")),
        EnvVarRestore::set("KELVIN_TELEGRAM_ALLOW_CHAT_IDS", Some("")),
        EnvVarRestore::set("KELVIN_TELEGRAM_MAX_MESSAGES_PER_MINUTE", Some("10")),
        EnvVarRestore::set("KELVIN_TELEGRAM_BOT_TOKEN", None),
    ];

    let (url, server_handle) = start_gateway(Some("secret")).await;
    let (mut socket, _) = connect_async(url).await.expect("connect");
    send_request(
        &mut socket,
        "connect-telegram",
        "connect",
        json!({
            "auth": {"token": "secret"},
            "client_id": "integration-test",
        }),
    )
    .await;
    let connect_response = read_until_response(&mut socket, "connect-telegram").await;
    assert_eq!(connect_response["ok"], json!(true));
    assert_eq!(
        connect_response["payload"]["protocol_version"],
        json!(GATEWAY_PROTOCOL_VERSION)
    );
    assert_eq!(
        connect_response["payload"]["supported_methods"],
        json!(GATEWAY_METHODS_V1)
    );

    send_request(
        &mut socket,
        "tg-ingest-1",
        "channel.telegram.ingest",
        json!({
            "delivery_id": "telegram-delivery-1",
            "chat_id": 42,
            "text": "hello from telegram",
            "timeout_ms": 3000
        }),
    )
    .await;
    let pairing_response = read_until_response(&mut socket, "tg-ingest-1").await;
    assert_eq!(pairing_response["ok"], json!(true));
    assert_eq!(
        pairing_response["payload"]["status"],
        json!("pairing_required")
    );
    let pairing_code = pairing_response["payload"]["pairing_code"]
        .as_str()
        .expect("pairing code")
        .to_string();

    send_request(
        &mut socket,
        "tg-pair-approve",
        "channel.telegram.pair.approve",
        json!({
            "code": pairing_code
        }),
    )
    .await;
    let approve_response = read_until_response(&mut socket, "tg-pair-approve").await;
    assert_eq!(approve_response["ok"], json!(true));
    assert_eq!(approve_response["payload"]["approved"], json!(true));

    send_request(
        &mut socket,
        "tg-ingest-2",
        "channel.telegram.ingest",
        json!({
            "delivery_id": "telegram-delivery-2",
            "chat_id": 42,
            "text": "what is KelvinClaw?",
            "timeout_ms": 3000
        }),
    )
    .await;
    let dispatch_response = read_until_response(&mut socket, "tg-ingest-2").await;
    assert_eq!(dispatch_response["ok"], json!(true));
    assert_eq!(dispatch_response["payload"]["status"], json!("completed"));
    assert!(dispatch_response["payload"]["response_text"]
        .as_str()
        .unwrap_or_default()
        .contains("Echo:"));

    send_request(
        &mut socket,
        "tg-ingest-dup",
        "channel.telegram.ingest",
        json!({
            "delivery_id": "telegram-delivery-2",
            "chat_id": 42,
            "text": "what is KelvinClaw?",
            "timeout_ms": 3000
        }),
    )
    .await;
    let dedupe_response = read_until_response(&mut socket, "tg-ingest-dup").await;
    assert_eq!(dedupe_response["ok"], json!(true));
    assert_eq!(dedupe_response["payload"]["status"], json!("deduped"));

    server_handle.abort();
}

#[tokio::test]
async fn gateway_slack_channel_dispatch_and_dedup_flow_works() {
    let _guard = ENV_LOCK.lock().await;
    let _env_restore = [
        EnvVarRestore::set("KELVIN_SLACK_ENABLED", Some("true")),
        EnvVarRestore::set("KELVIN_SLACK_INGRESS_TOKEN", Some("slack-ingress-secret")),
        EnvVarRestore::set("KELVIN_SLACK_BOT_TOKEN", None),
        EnvVarRestore::set("KELVIN_SLACK_MAX_MESSAGES_PER_MINUTE", Some("20")),
    ];

    let (url, server_handle) = start_gateway(Some("secret")).await;
    let (mut socket, _) = connect_async(url).await.expect("connect");
    send_request(
        &mut socket,
        "connect-slack",
        "connect",
        json!({
            "auth": {"token": "secret"},
            "client_id": "integration-test",
        }),
    )
    .await;
    let connect_response = read_until_response(&mut socket, "connect-slack").await;
    assert_eq!(connect_response["ok"], json!(true));

    send_request(
        &mut socket,
        "slack-auth-bad",
        "channel.slack.ingest",
        json!({
            "delivery_id": "slack-delivery-auth-bad",
            "channel_id": "C1",
            "user_id": "U1",
            "text": "hello",
            "auth_token": "wrong",
            "timeout_ms": 3000
        }),
    )
    .await;
    let auth_mismatch = read_until_response(&mut socket, "slack-auth-bad").await;
    assert_eq!(auth_mismatch["ok"], json!(false));
    assert_eq!(auth_mismatch["error"]["code"], json!("not_found"));

    send_request(
        &mut socket,
        "slack-ingest-1",
        "channel.slack.ingest",
        json!({
            "delivery_id": "slack-delivery-1",
            "channel_id": "C1",
            "user_id": "U1",
            "text": "what is kelvin?",
            "auth_token": "slack-ingress-secret",
            "timeout_ms": 3000
        }),
    )
    .await;
    let dispatch_response = read_until_response(&mut socket, "slack-ingest-1").await;
    assert_eq!(dispatch_response["ok"], json!(true));
    assert_eq!(dispatch_response["payload"]["status"], json!("completed"));
    assert_eq!(
        dispatch_response["payload"]["route"]["session_id"],
        json!("slack:C1")
    );

    send_request(
        &mut socket,
        "slack-ingest-dup",
        "channel.slack.ingest",
        json!({
            "delivery_id": "slack-delivery-1",
            "channel_id": "C1",
            "user_id": "U1",
            "text": "what is kelvin?",
            "auth_token": "slack-ingress-secret",
            "timeout_ms": 3000
        }),
    )
    .await;
    let dedupe_response = read_until_response(&mut socket, "slack-ingest-dup").await;
    assert_eq!(dedupe_response["ok"], json!(true));
    assert_eq!(dedupe_response["payload"]["status"], json!("deduped"));

    send_request(
        &mut socket,
        "slack-status",
        "channel.slack.status",
        json!({}),
    )
    .await;
    let status_response = read_until_response(&mut socket, "slack-status").await;
    assert_eq!(status_response["ok"], json!(true));
    assert_eq!(status_response["payload"]["enabled"], json!(true));
    assert!(
        status_response["payload"]["metrics"]["ingest_total"]
            .as_u64()
            .unwrap_or_default()
            >= 2
    );

    server_handle.abort();
}

#[tokio::test]
async fn gateway_discord_channel_flood_controls_and_route_inspection_work() {
    let _guard = ENV_LOCK.lock().await;
    let _env_restore = [
        EnvVarRestore::set("KELVIN_DISCORD_ENABLED", Some("true")),
        EnvVarRestore::set("KELVIN_DISCORD_BOT_TOKEN", None),
        EnvVarRestore::set("KELVIN_DISCORD_MAX_MESSAGES_PER_MINUTE", Some("1")),
        EnvVarRestore::set(
            "KELVIN_CHANNEL_ROUTING_RULES_JSON",
            Some(
                r#"[
                {"id":"discord-priority","priority":50,"channel":"discord","account_id":"D1","route_session_id":"discord-priority-session","route_system_prompt":"route:discord"},
                {"id":"discord-fallback","priority":10,"channel":"discord","route_session_id":"discord-fallback-session"}
            ]"#,
            ),
        ),
    ];

    let (url, server_handle) = start_gateway(Some("secret")).await;
    let (mut socket, _) = connect_async(url).await.expect("connect");
    send_request(
        &mut socket,
        "connect-discord",
        "connect",
        json!({
            "auth": {"token": "secret"},
            "client_id": "integration-test",
        }),
    )
    .await;
    let connect_response = read_until_response(&mut socket, "connect-discord").await;
    assert_eq!(connect_response["ok"], json!(true));

    send_request(
        &mut socket,
        "route-discord",
        "channel.route.inspect",
        json!({
            "channel": "discord",
            "account_id": "D1",
            "sender_tier": "standard"
        }),
    )
    .await;
    let route_response = read_until_response(&mut socket, "route-discord").await;
    assert_eq!(route_response["ok"], json!(true));
    assert_eq!(
        route_response["payload"]["route"]["matched_rule_id"],
        json!("discord-priority")
    );
    assert_eq!(
        route_response["payload"]["route"]["session_id"],
        json!("discord-priority-session")
    );

    send_request(
        &mut socket,
        "discord-ingest-1",
        "channel.discord.ingest",
        json!({
            "delivery_id": "discord-delivery-1",
            "channel_id": "D1",
            "user_id": "U1",
            "text": "first discord message",
            "timeout_ms": 3000
        }),
    )
    .await;
    let first_response = read_until_response(&mut socket, "discord-ingest-1").await;
    assert_eq!(first_response["ok"], json!(true));
    assert_eq!(first_response["payload"]["status"], json!("completed"));
    assert_eq!(
        first_response["payload"]["route"]["session_id"],
        json!("discord-priority-session")
    );

    send_request(
        &mut socket,
        "discord-ingest-2",
        "channel.discord.ingest",
        json!({
            "delivery_id": "discord-delivery-2",
            "channel_id": "D1",
            "user_id": "U1",
            "text": "second discord message",
            "timeout_ms": 3000
        }),
    )
    .await;
    let second_response = read_until_response(&mut socket, "discord-ingest-2").await;
    assert_eq!(second_response["ok"], json!(false));
    assert_eq!(second_response["error"]["code"], json!("timeout"));

    send_request(
        &mut socket,
        "discord-status",
        "channel.discord.status",
        json!({}),
    )
    .await;
    let status_response = read_until_response(&mut socket, "discord-status").await;
    assert_eq!(status_response["ok"], json!(true));
    assert_eq!(status_response["payload"]["enabled"], json!(true));
    assert!(
        status_response["payload"]["metrics"]["rate_limited_total"]
            .as_u64()
            .unwrap_or_default()
            >= 1
    );

    server_handle.abort();
}
