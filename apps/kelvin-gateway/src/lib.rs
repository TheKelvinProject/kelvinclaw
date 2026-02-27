mod channels;

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use channels::{
    ChannelEngine, ChannelRouteInspectRequest, DiscordIngressRequest, SlackIngressRequest,
    TelegramIngressRequest, TelegramPairApproveRequest,
};
use futures_util::{SinkExt, StreamExt};
use kelvin_core::{now_ms, KelvinError, RunOutcome};
use kelvin_sdk::{
    KelvinSdkAcceptedRun, KelvinSdkRunRequest, KelvinSdkRuntime, KelvinSdkRuntimeConfig,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio::time::Duration;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

pub const GATEWAY_PROTOCOL_VERSION: &str = "1.0.0";
pub const GATEWAY_METHODS_V1: &[&str] = &[
    "agent",
    "agent.outcome",
    "agent.state",
    "agent.wait",
    "channel.discord.ingest",
    "channel.discord.status",
    "channel.route.inspect",
    "channel.slack.ingest",
    "channel.slack.status",
    "channel.telegram.ingest",
    "channel.telegram.pair.approve",
    "channel.telegram.status",
    "connect",
    "health",
    "run.outcome",
    "run.state",
    "run.submit",
    "run.wait",
];

#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub bind_addr: SocketAddr,
    pub auth_token: Option<String>,
    pub runtime: KelvinSdkRuntimeConfig,
}

#[derive(Clone)]
struct GatewayState {
    runtime: KelvinSdkRuntime,
    auth_token: Option<String>,
    started_at: Instant,
    idempotency: Arc<Mutex<IdempotencyCache>>,
    channels: Arc<Mutex<ChannelEngine>>,
}

#[derive(Debug, Clone)]
struct CachedAgentAcceptance {
    run_id: String,
    accepted_at_ms: u128,
    cli_plugin_preflight: Option<String>,
}

#[derive(Debug, Clone)]
struct IdempotencyCache {
    max_entries: usize,
    map: HashMap<String, CachedAgentAcceptance>,
    order: VecDeque<String>,
}

impl IdempotencyCache {
    fn new(max_entries: usize) -> Self {
        Self {
            max_entries,
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&self, request_id: &str) -> Option<CachedAgentAcceptance> {
        self.map.get(request_id).cloned()
    }

    fn insert(&mut self, request_id: String, acceptance: CachedAgentAcceptance) {
        if let std::collections::hash_map::Entry::Occupied(mut entry) =
            self.map.entry(request_id.clone())
        {
            entry.insert(acceptance);
            return;
        }

        if self.max_entries > 0 && self.order.len() >= self.max_entries {
            if let Some(evicted) = self.order.pop_front() {
                self.map.remove(&evicted);
            }
        }

        self.order.push_back(request_id.clone());
        self.map.insert(request_id, acceptance);
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ClientFrame {
    Req {
        id: String,
        method: String,
        #[serde(default)]
        params: Value,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerFrame {
    Res {
        id: String,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        payload: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<GatewayErrorPayload>,
    },
    Event {
        event: String,
        payload: Value,
    },
}

#[derive(Debug, Serialize)]
struct GatewayErrorPayload {
    code: String,
    message: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectParams {
    auth: Option<ConnectAuth>,
    client_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectAuth {
    token: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentParams {
    request_id: String,
    prompt: String,
    session_id: Option<String>,
    workspace_dir: Option<String>,
    timeout_ms: Option<u64>,
    system_prompt: Option<String>,
    memory_query: Option<String>,
    run_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunWaitParams {
    run_id: String,
    timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunStateParams {
    run_id: String,
}

#[derive(Debug, Clone)]
pub struct GatewayDoctorConfig {
    pub endpoint: String,
    pub auth_token: Option<String>,
    pub plugin_home: PathBuf,
    pub trust_policy_path: PathBuf,
    pub timeout_ms: u64,
}

pub async fn run_gateway_doctor(config: GatewayDoctorConfig) -> Result<Value, String> {
    let plugin_home_ok = config.plugin_home.is_dir();
    let trust_policy_parse_ok = config.trust_policy_path.is_file()
        && fs::read(&config.trust_policy_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some();

    let mut ws_ok = false;
    let mut connect_ok = false;
    let mut health_ok = false;
    let mut ws_error: Option<String> = None;
    let mut connect_error: Option<String> = None;
    let mut health_error: Option<String> = None;
    let mut doctor_errors = Vec::new();
    let mut checks = Vec::new();

    let connect_result = tokio::time::timeout(
        Duration::from_millis(config.timeout_ms.max(250)),
        connect_async(config.endpoint.clone()),
    )
    .await;
    match connect_result {
        Ok(Ok((mut socket, _))) => {
            ws_ok = true;
            let connect_payload = json!({
                "type": "req",
                "id": "doctor-connect",
                "method": "connect",
                "params": {
                    "auth": config.auth_token.as_ref().map(|token| json!({ "token": token })),
                    "client_id": "kelvin-doctor"
                }
            });
            if socket
                .send(Message::Text(connect_payload.to_string()))
                .await
                .is_err()
            {
                let message = "failed to send connect request".to_string();
                connect_error = Some(message.clone());
                doctor_errors.push(message);
            } else if let Ok(response) = wait_for_response(&mut socket, "doctor-connect").await {
                connect_ok = response
                    .get("ok")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                if !connect_ok {
                    let message = response
                        .get("error")
                        .and_then(|value| value.get("message"))
                        .and_then(|value| value.as_str())
                        .unwrap_or("connect failed")
                        .to_string();
                    connect_error = Some(message.clone());
                    doctor_errors.push(message);
                } else {
                    let health_payload = json!({
                        "type": "req",
                        "id": "doctor-health",
                        "method": "health",
                        "params": {}
                    });
                    if socket
                        .send(Message::Text(health_payload.to_string()))
                        .await
                        .is_err()
                    {
                        let message = "failed to send health request".to_string();
                        health_error = Some(message.clone());
                        doctor_errors.push(message);
                    } else if let Ok(health_response) =
                        wait_for_response(&mut socket, "doctor-health").await
                    {
                        health_ok = health_response
                            .get("ok")
                            .and_then(|value| value.as_bool())
                            .unwrap_or(false);
                        if !health_ok {
                            let message = health_response
                                .get("error")
                                .and_then(|value| value.get("message"))
                                .and_then(|value| value.as_str())
                                .unwrap_or("health check failed")
                                .to_string();
                            health_error = Some(message.clone());
                            doctor_errors.push(message);
                        }
                    } else {
                        let message = "missing health response from gateway".to_string();
                        health_error = Some(message.clone());
                        doctor_errors.push(message);
                    }
                }
            } else {
                let message = "missing connect response from gateway".to_string();
                connect_error = Some(message.clone());
                doctor_errors.push(message);
            }
            let _ = socket.close(None).await;
        }
        Ok(Err(err)) => {
            let message = format!("websocket connect failed: {err}");
            ws_error = Some(message.clone());
            doctor_errors.push(message);
        }
        Err(_) => {
            let message = "websocket connect timed out".to_string();
            ws_error = Some(message.clone());
            doctor_errors.push(message);
        }
    }

    checks.push(build_doctor_check(
        "plugin_home",
        plugin_home_ok,
        if plugin_home_ok {
            format!(
                "plugin home exists: {}",
                config.plugin_home.to_string_lossy()
            )
        } else {
            format!(
                "plugin home is missing: {}",
                config.plugin_home.to_string_lossy()
            )
        },
        "create the plugin home and install required plugins, for example: scripts/kelvin-setup.sh --force",
    ));
    checks.push(build_doctor_check(
        "trust_policy",
        trust_policy_parse_ok,
        if trust_policy_parse_ok {
            format!(
                "trust policy is present and valid JSON: {}",
                config.trust_policy_path.to_string_lossy()
            )
        } else {
            format!(
                "trust policy missing or invalid JSON: {}",
                config.trust_policy_path.to_string_lossy()
            )
        },
        "install plugins again to refresh trust policy, or provide --trust-policy <path> with a valid trusted_publishers.json",
    ));
    checks.push(build_doctor_check(
        "websocket_connect",
        ws_ok,
        ws_error.unwrap_or_else(|| "gateway websocket endpoint reachable".to_string()),
        "start the gateway daemon and verify endpoint/token, for example: scripts/kelvin-gateway-daemon.sh start",
    ));
    checks.push(build_doctor_check(
        "gateway_connect_handshake",
        connect_ok,
        connect_error.unwrap_or_else(|| "gateway connect handshake succeeded".to_string()),
        "verify gateway auth token and connect method parameters, then rerun scripts/kelvin-doctor.sh",
    ));
    checks.push(build_doctor_check(
        "gateway_health",
        health_ok,
        health_error.unwrap_or_else(|| "gateway health check succeeded".to_string()),
        "inspect daemon logs and runtime state (scripts/kelvin-gateway-daemon.sh logs), then fix reported runtime errors",
    ));

    let failed = checks
        .iter()
        .filter(|item| item.get("status") != Some(&json!("pass")))
        .count();
    let ok = failed == 0;
    Ok(json!({
        "ok": ok,
        "summary": {
            "passed": checks.len().saturating_sub(failed),
            "failed": failed,
            "checked_at_ms": now_ms()
        },
        "checks": checks,
        "legacy_checks": {
            "plugin_home_ok": plugin_home_ok,
            "trust_policy_ok": trust_policy_parse_ok,
            "websocket_connect_ok": ws_ok,
            "connect_ok": connect_ok,
            "health_ok": health_ok
        },
        "inputs": {
            "endpoint": config.endpoint,
            "plugin_home": config.plugin_home,
            "trust_policy_path": config.trust_policy_path
        },
        "errors": doctor_errors
    }))
}

fn build_doctor_check(id: &str, ok: bool, message: String, remediation: &str) -> Value {
    json!({
        "id": id,
        "status": if ok { "pass" } else { "fail" },
        "severity": if ok { "info" } else { "error" },
        "message": message,
        "remediation": remediation
    })
}

async fn wait_for_response(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
    target_id: &str,
) -> Result<Value, String> {
    while let Some(message) = socket.next().await {
        let message = message.map_err(|err| err.to_string())?;
        let Message::Text(text) = message else {
            continue;
        };
        let frame: Value = serde_json::from_str(&text).map_err(|err| err.to_string())?;
        if frame.get("type") == Some(&json!("res")) && frame.get("id") == Some(&json!(target_id)) {
            return Ok(frame);
        }
    }
    Err("connection closed before response".to_string())
}

pub async fn run_gateway(config: GatewayConfig) -> Result<(), String> {
    let listener = TcpListener::bind(config.bind_addr)
        .await
        .map_err(|err| format!("bind failed on {}: {err}", config.bind_addr))?;
    let runtime = KelvinSdkRuntime::initialize(config.runtime)
        .await
        .map_err(|err| err.to_string())?;
    run_gateway_with_listener(listener, runtime, config.auth_token).await
}

pub async fn run_gateway_with_listener(
    listener: TcpListener,
    runtime: KelvinSdkRuntime,
    auth_token: Option<String>,
) -> Result<(), String> {
    let local_addr = listener
        .local_addr()
        .map_err(|err| format!("local_addr failed: {err}"))?;
    println!("kelvin-gateway listening on ws://{local_addr}");
    let channels =
        ChannelEngine::from_env().map_err(|err| format!("initialize channel engine: {err}"))?;

    let state = GatewayState {
        runtime,
        auth_token: auth_token.map(|value| value.trim().to_string()),
        started_at: Instant::now(),
        idempotency: Arc::new(Mutex::new(IdempotencyCache::new(2_048))),
        channels: Arc::new(Mutex::new(channels)),
    };

    loop {
        let (stream, peer) = listener
            .accept()
            .await
            .map_err(|err| format!("accept failed: {err}"))?;
        let connection_state = state.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_connection(stream, connection_state).await {
                eprintln!("gateway connection error for {peer}: {err}");
            }
        });
    }
}

async fn handle_connection(stream: TcpStream, state: GatewayState) -> Result<(), String> {
    let ws_stream = tokio_tungstenite::accept_async(stream)
        .await
        .map_err(|err| format!("websocket upgrade failed: {err}"))?;
    let (mut sink, mut source) = ws_stream.split();
    let (writer_tx, mut writer_rx) = mpsc::unbounded_channel::<Message>();

    let writer_task = tokio::spawn(async move {
        while let Some(message) = writer_rx.recv().await {
            if sink.send(message).await.is_err() {
                break;
            }
        }
    });

    let first_message = match source.next().await {
        Some(Ok(Message::Text(text))) => text,
        Some(Ok(_)) => {
            let _ = send_error(
                &writer_tx,
                "",
                "handshake_required",
                "first frame must be a connect request",
            );
            let _ = writer_tx.send(Message::Close(None));
            drop(writer_tx);
            let _ = writer_task.await;
            return Ok(());
        }
        Some(Err(err)) => {
            writer_task.abort();
            return Err(format!("receive failed: {err}"));
        }
        None => {
            writer_task.abort();
            return Ok(());
        }
    };

    let ClientFrame::Req {
        id: first_id,
        method: first_method,
        params: first_params,
    } = parse_client_frame(&first_message)?;

    if first_method != "connect" {
        let _ = send_error(
            &writer_tx,
            &first_id,
            "handshake_required",
            "first method must be connect",
        );
        let _ = writer_tx.send(Message::Close(None));
        drop(writer_tx);
        let _ = writer_task.await;
        return Ok(());
    }

    let connect_params: ConnectParams = match parse_params(first_params, "connect") {
        Ok(params) => params,
        Err(err) => {
            let _ = send_gateway_error(&writer_tx, &first_id, err);
            let _ = writer_tx.send(Message::Close(None));
            drop(writer_tx);
            let _ = writer_task.await;
            return Ok(());
        }
    };
    let _client_id = connect_params
        .client_id
        .unwrap_or_else(|| "unknown".to_string());
    if let Err(err) = verify_auth_token(state.auth_token.as_deref(), connect_params.auth.as_ref()) {
        let _ = send_gateway_error(&writer_tx, &first_id, err);
        let _ = writer_tx.send(Message::Close(None));
        drop(writer_tx);
        let _ = writer_task.await;
        return Ok(());
    }
    send_ok(
        &writer_tx,
        &first_id,
        json!({
            "status": "connected",
            "protocol_version": GATEWAY_PROTOCOL_VERSION,
            "supported_methods": GATEWAY_METHODS_V1,
            "server_time_ms": now_ms(),
            "loaded_installed_plugins": state.runtime.loaded_installed_plugins(),
        }),
    )?;

    let mut event_rx = state.runtime.subscribe_events();
    let event_writer = writer_tx.clone();
    let event_task = tokio::spawn(async move {
        loop {
            match event_rx.recv().await {
                Ok(event) => {
                    if send_event(&event_writer, &event).is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    while let Some(message) = source.next().await {
        match message {
            Ok(Message::Text(text)) => {
                let frame = parse_client_frame(&text)?;
                let ClientFrame::Req { id, method, params } = frame;
                if method == "connect" {
                    send_error(
                        &writer_tx,
                        &id,
                        "invalid_request",
                        "connect can only be sent once per socket",
                    )?;
                    continue;
                }
                if !is_supported_method(&method) {
                    send_error(
                        &writer_tx,
                        &id,
                        "method_not_found",
                        &format!("unknown method: {method}"),
                    )?;
                    continue;
                }
                match handle_request(&state, &id, &method, params).await {
                    Ok(payload) => send_ok(&writer_tx, &id, payload)?,
                    Err(err) => send_gateway_error(&writer_tx, &id, err)?,
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(err) => {
                event_task.abort();
                writer_task.abort();
                return Err(format!("socket read failed: {err}"));
            }
        }
    }

    event_task.abort();
    drop(writer_tx);
    let _ = writer_task.await;
    Ok(())
}

async fn handle_request(
    state: &GatewayState,
    _request_id: &str,
    method: &str,
    params: Value,
) -> Result<Value, GatewayErrorPayload> {
    match method {
        "health" => Ok(json!({
            "status": "ok",
            "protocol_version": GATEWAY_PROTOCOL_VERSION,
            "supported_methods": GATEWAY_METHODS_V1,
            "uptime_ms": state.started_at.elapsed().as_millis(),
            "loaded_installed_plugins": state.runtime.loaded_installed_plugins(),
            "channels": {
                "routing": state.channels.lock().await.routing_status(),
                "telegram": state.channels.lock().await.telegram_status(),
                "slack": state.channels.lock().await.slack_status(),
                "discord": state.channels.lock().await.discord_status(),
            },
        })),
        "agent" | "run.submit" => {
            let params: AgentParams = parse_params(params, method)?;
            submit_agent(state, params).await
        }
        "agent.wait" | "run.wait" => {
            let params: RunWaitParams = parse_params(params, method)?;
            let wait = state
                .runtime
                .wait(&params.run_id, params.timeout_ms.unwrap_or(30_000))
                .await
                .map_err(map_kelvin_error)?;
            Ok(serde_json::to_value(wait).unwrap_or_else(|_| json!({})))
        }
        "agent.state" | "run.state" => {
            let params: RunStateParams = parse_params(params, method)?;
            let run_state = state
                .runtime
                .state(&params.run_id)
                .await
                .map_err(map_kelvin_error)?;
            Ok(serde_json::to_value(run_state).unwrap_or_else(|_| json!({})))
        }
        "agent.outcome" | "run.outcome" => {
            let params: RunWaitParams = parse_params(params, method)?;
            let outcome = state
                .runtime
                .wait_for_outcome(&params.run_id, params.timeout_ms.unwrap_or(30_000))
                .await
                .map_err(map_kelvin_error)?;
            match outcome {
                RunOutcome::Completed(result) => Ok(json!({
                    "status": "completed",
                    "result": result,
                })),
                RunOutcome::Failed(error) => Ok(json!({
                    "status": "failed",
                    "error": error,
                })),
                RunOutcome::Timeout => Ok(json!({
                    "status": "timeout",
                })),
            }
        }
        "channel.telegram.ingest" => {
            let params: TelegramIngressRequest = parse_params(params, method)?;
            let mut channels = state.channels.lock().await;
            channels
                .telegram_ingest(&state.runtime, params)
                .await
                .map_err(map_kelvin_error)
        }
        "channel.telegram.pair.approve" => {
            let params: TelegramPairApproveRequest = parse_params(params, method)?;
            let mut channels = state.channels.lock().await;
            channels
                .telegram_approve_pairing(&params.code)
                .map_err(map_kelvin_error)
        }
        "channel.telegram.status" => {
            let channels = state.channels.lock().await;
            Ok(channels.telegram_status())
        }
        "channel.slack.ingest" => {
            let params: SlackIngressRequest = parse_params(params, method)?;
            let mut channels = state.channels.lock().await;
            channels
                .slack_ingest(&state.runtime, params)
                .await
                .map_err(map_kelvin_error)
        }
        "channel.slack.status" => {
            let channels = state.channels.lock().await;
            Ok(channels.slack_status())
        }
        "channel.discord.ingest" => {
            let params: DiscordIngressRequest = parse_params(params, method)?;
            let mut channels = state.channels.lock().await;
            channels
                .discord_ingest(&state.runtime, params)
                .await
                .map_err(map_kelvin_error)
        }
        "channel.discord.status" => {
            let channels = state.channels.lock().await;
            Ok(channels.discord_status())
        }
        "channel.route.inspect" => {
            let params: ChannelRouteInspectRequest = parse_params(params, method)?;
            let channels = state.channels.lock().await;
            channels.route_inspect(params).map_err(map_kelvin_error)
        }
        _ => Err(GatewayErrorPayload {
            code: "method_not_found".to_string(),
            message: format!("unknown method: {method}"),
        }),
    }
}

async fn submit_agent(
    state: &GatewayState,
    params: AgentParams,
) -> Result<Value, GatewayErrorPayload> {
    let request_id = params.request_id.trim();
    if request_id.is_empty() {
        return Err(GatewayErrorPayload {
            code: "invalid_input".to_string(),
            message: "request_id must not be empty".to_string(),
        });
    }

    if let Some(cached) = state.idempotency.lock().await.get(request_id) {
        return Ok(json!({
            "run_id": cached.run_id,
            "status": "accepted",
            "accepted_at_ms": cached.accepted_at_ms,
            "deduped": true,
            "cli_plugin_preflight": cached.cli_plugin_preflight,
        }));
    }

    let accepted: KelvinSdkAcceptedRun = state
        .runtime
        .submit(KelvinSdkRunRequest {
            prompt: params.prompt,
            session_id: params.session_id,
            workspace_dir: params.workspace_dir.map(PathBuf::from),
            timeout_ms: params.timeout_ms,
            system_prompt: params.system_prompt,
            memory_query: params.memory_query,
            run_id: params.run_id,
        })
        .await
        .map_err(map_kelvin_error)?;

    let cached = CachedAgentAcceptance {
        run_id: accepted.run_id.clone(),
        accepted_at_ms: accepted.accepted_at_ms,
        cli_plugin_preflight: accepted.cli_plugin_preflight.clone(),
    };
    state
        .idempotency
        .lock()
        .await
        .insert(request_id.to_string(), cached);

    Ok(json!({
        "run_id": accepted.run_id,
        "status": "accepted",
        "accepted_at_ms": accepted.accepted_at_ms,
        "deduped": false,
        "cli_plugin_preflight": accepted.cli_plugin_preflight,
    }))
}

fn is_supported_method(method: &str) -> bool {
    GATEWAY_METHODS_V1.contains(&method)
}

fn verify_auth_token(
    required_token: Option<&str>,
    provided_auth: Option<&ConnectAuth>,
) -> Result<(), GatewayErrorPayload> {
    let Some(required_token) = required_token else {
        return Ok(());
    };

    let Some(provided) = provided_auth else {
        return Err(GatewayErrorPayload {
            code: "unauthorized".to_string(),
            message: "missing auth token".to_string(),
        });
    };
    if provided.token != required_token {
        return Err(GatewayErrorPayload {
            code: "unauthorized".to_string(),
            message: "invalid auth token".to_string(),
        });
    }
    Ok(())
}

fn parse_client_frame(raw: &str) -> Result<ClientFrame, String> {
    serde_json::from_str::<ClientFrame>(raw).map_err(|err| format!("invalid frame: {err}"))
}

fn parse_params<T>(params: Value, method: &str) -> Result<T, GatewayErrorPayload>
where
    T: DeserializeOwned,
{
    serde_json::from_value(params).map_err(|err| GatewayErrorPayload {
        code: "invalid_input".to_string(),
        message: format!("invalid params for {method}: {err}"),
    })
}

fn map_kelvin_error(err: KelvinError) -> GatewayErrorPayload {
    let code = match err {
        KelvinError::InvalidInput(_) => "invalid_input",
        KelvinError::NotFound(_) => "not_found",
        KelvinError::Timeout(_) => "timeout",
        KelvinError::Backend(_) => "backend_error",
        KelvinError::Io(_) => "io_error",
    };
    GatewayErrorPayload {
        code: code.to_string(),
        message: err.to_string(),
    }
}

fn send_ok(
    writer_tx: &mpsc::UnboundedSender<Message>,
    id: &str,
    payload: Value,
) -> Result<(), String> {
    let frame = ServerFrame::Res {
        id: id.to_string(),
        ok: true,
        payload: Some(payload),
        error: None,
    };
    send_frame(writer_tx, frame)
}

fn send_error(
    writer_tx: &mpsc::UnboundedSender<Message>,
    id: &str,
    code: &str,
    message: &str,
) -> Result<(), String> {
    send_gateway_error(
        writer_tx,
        id,
        GatewayErrorPayload {
            code: code.to_string(),
            message: message.to_string(),
        },
    )
}

fn send_gateway_error(
    writer_tx: &mpsc::UnboundedSender<Message>,
    id: &str,
    error: GatewayErrorPayload,
) -> Result<(), String> {
    let frame = ServerFrame::Res {
        id: id.to_string(),
        ok: false,
        payload: None,
        error: Some(error),
    };
    send_frame(writer_tx, frame)
}

fn send_event(
    writer_tx: &mpsc::UnboundedSender<Message>,
    event: &kelvin_core::AgentEvent,
) -> Result<(), String> {
    let payload = serde_json::to_value(event).map_err(|err| err.to_string())?;
    let frame = ServerFrame::Event {
        event: "agent".to_string(),
        payload,
    };
    send_frame(writer_tx, frame)
}

fn send_frame(
    writer_tx: &mpsc::UnboundedSender<Message>,
    frame: ServerFrame,
) -> Result<(), String> {
    let text = serde_json::to_string(&frame).map_err(|err| err.to_string())?;
    writer_tx
        .send(Message::Text(text))
        .map_err(|_| "connection closed".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn idempotency_cache_evicts_oldest_entry() {
        let mut cache = IdempotencyCache::new(2);
        cache.insert(
            "a".to_string(),
            CachedAgentAcceptance {
                run_id: "run-a".to_string(),
                accepted_at_ms: 1,
                cli_plugin_preflight: None,
            },
        );
        cache.insert(
            "b".to_string(),
            CachedAgentAcceptance {
                run_id: "run-b".to_string(),
                accepted_at_ms: 2,
                cli_plugin_preflight: None,
            },
        );
        cache.insert(
            "c".to_string(),
            CachedAgentAcceptance {
                run_id: "run-c".to_string(),
                accepted_at_ms: 3,
                cli_plugin_preflight: None,
            },
        );

        assert!(cache.get("a").is_none());
        assert_eq!(cache.get("b").expect("b").run_id, "run-b");
        assert_eq!(cache.get("c").expect("c").run_id, "run-c");
    }

    #[test]
    fn gateway_protocol_version_is_stable() {
        assert_eq!(GATEWAY_PROTOCOL_VERSION, "1.0.0");
    }

    #[test]
    fn gateway_method_contract_matches_v1_surface() {
        let methods = GATEWAY_METHODS_V1.to_vec();
        assert_eq!(
            methods,
            vec![
                "agent",
                "agent.outcome",
                "agent.state",
                "agent.wait",
                "channel.discord.ingest",
                "channel.discord.status",
                "channel.route.inspect",
                "channel.slack.ingest",
                "channel.slack.status",
                "channel.telegram.ingest",
                "channel.telegram.pair.approve",
                "channel.telegram.status",
                "connect",
                "health",
                "run.outcome",
                "run.state",
                "run.submit",
                "run.wait",
            ]
        );
        let unique = methods.iter().copied().collect::<HashSet<_>>();
        assert_eq!(
            unique.len(),
            methods.len(),
            "duplicate method names in contract"
        );
        for method in methods {
            assert!(is_supported_method(method), "missing method from allowlist");
        }
    }

    #[tokio::test]
    async fn doctor_report_is_machine_readable_and_actionable() {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_millis())
            .unwrap_or_default();
        let temp_root = std::env::temp_dir().join(format!("kelvin-doctor-test-{millis}"));
        let plugin_home = temp_root.join("plugins");
        std::fs::create_dir_all(&plugin_home).expect("create plugin home");
        let trust_policy_path = temp_root.join("trusted_publishers.json");
        std::fs::write(
            &trust_policy_path,
            b"{\"require_signature\":true,\"publishers\":[]}",
        )
        .expect("write trust policy");

        let report = run_gateway_doctor(GatewayDoctorConfig {
            endpoint: "ws://127.0.0.1:1".to_string(),
            auth_token: None,
            plugin_home,
            trust_policy_path,
            timeout_ms: 250,
        })
        .await
        .expect("doctor report");

        assert!(report.get("ok").and_then(|item| item.as_bool()).is_some());
        let checks = report
            .get("checks")
            .and_then(|item| item.as_array())
            .expect("checks array");
        assert!(!checks.is_empty(), "checks should not be empty");
        for check in checks {
            assert!(check.get("id").is_some(), "missing check id");
            assert!(check.get("status").is_some(), "missing check status");
            assert!(
                check.get("remediation").is_some(),
                "missing remediation hint"
            );
        }
    }
}
