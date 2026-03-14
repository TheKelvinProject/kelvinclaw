#![recursion_limit = "256"]

mod channels;
mod ingress;
mod scheduler;

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::BufReader;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use channels::{
    ChannelEngine, ChannelRouteInspectRequest, DiscordIngressRequest, SlackIngressRequest,
    TelegramIngressRequest, TelegramPairApproveRequest,
};
use futures_util::{SinkExt, StreamExt};
pub use ingress::GatewayIngressConfig;
use kelvin_core::{now_ms, KelvinError, RunOutcome};
use kelvin_sdk::{
    KelvinSdkAcceptedRun, KelvinSdkRunRequest, KelvinSdkRuntime, KelvinSdkRuntimeConfig,
};
use scheduler::{GatewayScheduler, ScheduleHistoryParams, ScheduleListParams};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, Mutex, Semaphore};
use tokio::time::{self, Duration};
use tokio_rustls::rustls::{self, pki_types::CertificateDer, pki_types::PrivateKeyDer};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::{self, Message};

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
    "schedule.history",
    "schedule.list",
];

const DEFAULT_MAX_CONNECTIONS: usize = 128;
const DEFAULT_MAX_MESSAGE_BYTES: usize = 64 * 1024;
const DEFAULT_MAX_FRAME_BYTES: usize = 16 * 1024;
const DEFAULT_HANDSHAKE_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_AUTH_FAILURE_THRESHOLD: u32 = 3;
const DEFAULT_AUTH_FAILURE_BACKOFF_MS: u64 = 1_500;
const DEFAULT_MAX_OUTBOUND_MESSAGES_PER_CONNECTION: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayTlsConfig {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewaySecurityConfig {
    pub tls: Option<GatewayTlsConfig>,
    pub allow_insecure_public_bind: bool,
    pub max_connections: usize,
    pub max_message_size_bytes: usize,
    pub max_frame_size_bytes: usize,
    pub handshake_timeout_ms: u64,
    pub auth_failure_threshold: u32,
    pub auth_failure_backoff_ms: u64,
    pub max_outbound_messages_per_connection: usize,
}

impl Default for GatewaySecurityConfig {
    fn default() -> Self {
        Self {
            tls: None,
            allow_insecure_public_bind: false,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            max_message_size_bytes: DEFAULT_MAX_MESSAGE_BYTES,
            max_frame_size_bytes: DEFAULT_MAX_FRAME_BYTES,
            handshake_timeout_ms: DEFAULT_HANDSHAKE_TIMEOUT_MS,
            auth_failure_threshold: DEFAULT_AUTH_FAILURE_THRESHOLD,
            auth_failure_backoff_ms: DEFAULT_AUTH_FAILURE_BACKOFF_MS,
            max_outbound_messages_per_connection: DEFAULT_MAX_OUTBOUND_MESSAGES_PER_CONNECTION,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub bind_addr: SocketAddr,
    pub auth_token: Option<String>,
    pub runtime: KelvinSdkRuntimeConfig,
    pub security: GatewaySecurityConfig,
    pub ingress: GatewayIngressConfig,
}

#[derive(Clone)]
struct GatewayState {
    bind_addr: SocketAddr,
    tls_enabled: bool,
    ingress: Option<ingress::GatewayIngressRuntime>,
    runtime: KelvinSdkRuntime,
    auth_token: Option<String>,
    security: GatewaySecurityConfig,
    started_at: Instant,
    idempotency: Arc<Mutex<IdempotencyCache>>,
    channels: Arc<Mutex<ChannelEngine>>,
    scheduler: Arc<GatewayScheduler>,
    auth_failures: Arc<Mutex<AuthFailureTracker>>,
    connection_semaphore: Arc<Semaphore>,
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

#[derive(Debug, Clone, Copy)]
struct AuthFailureEntry {
    failures: u32,
    blocked_until_ms: u128,
}

#[derive(Debug, Default)]
struct AuthFailureTracker {
    max_entries: usize,
    map: HashMap<IpAddr, AuthFailureEntry>,
    order: VecDeque<IpAddr>,
}

impl AuthFailureTracker {
    fn new(max_entries: usize) -> Self {
        Self {
            max_entries: max_entries.max(32),
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn backoff_remaining_ms(&mut self, peer_ip: IpAddr) -> Option<u64> {
        let now = now_ms();
        let entry = self.map.get_mut(&peer_ip)?;
        if entry.blocked_until_ms <= now {
            entry.blocked_until_ms = 0;
            return None;
        }
        let remaining = entry.blocked_until_ms.saturating_sub(now);
        Some(remaining.min(u128::from(u64::MAX)) as u64)
    }

    fn record_failure(&mut self, peer_ip: IpAddr, security: &GatewaySecurityConfig) {
        let now = now_ms();
        let mut entry = self.map.remove(&peer_ip).unwrap_or(AuthFailureEntry {
            failures: 0,
            blocked_until_ms: 0,
        });
        entry.failures = entry.failures.saturating_add(1);
        if entry.failures >= security.auth_failure_threshold {
            let multiplier = u64::from(
                entry
                    .failures
                    .saturating_sub(security.auth_failure_threshold)
                    .saturating_add(1),
            );
            entry.blocked_until_ms = now.saturating_add(
                u128::from(security.auth_failure_backoff_ms) * u128::from(multiplier),
            );
        }
        self.touch(peer_ip, entry);
    }

    fn clear(&mut self, peer_ip: IpAddr) {
        self.map.remove(&peer_ip);
        self.order.retain(|ip| *ip != peer_ip);
    }

    fn touch(&mut self, peer_ip: IpAddr, entry: AuthFailureEntry) {
        self.order.retain(|ip| *ip != peer_ip);
        if self.max_entries > 0 && self.order.len() >= self.max_entries {
            if let Some(evicted) = self.order.pop_front() {
                self.map.remove(&evicted);
            }
        }
        self.order.push_back(peer_ip);
        self.map.insert(peer_ip, entry);
    }
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
    let mut security_check: Option<(bool, String)> = None;
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
                        } else if let Some(security) = health_response
                            .get("payload")
                            .and_then(|value| value.get("security"))
                        {
                            let bind_scope = security
                                .get("bind_scope")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown");
                            let tls_enabled = security
                                .get("tls_enabled")
                                .and_then(Value::as_bool)
                                .unwrap_or(false);
                            let insecure_override = security
                                .get("allow_insecure_public_bind")
                                .and_then(Value::as_bool)
                                .unwrap_or(false);
                            let transport = security
                                .get("transport")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown");
                            let ok = bind_scope != "public" || tls_enabled || insecure_override;
                            let message = if bind_scope == "public" && tls_enabled {
                                format!("gateway public bind is protected by {}", transport)
                            } else if bind_scope == "public" && insecure_override {
                                "gateway public bind is using an explicit insecure override"
                                    .to_string()
                            } else {
                                format!("gateway bind scope is {}", bind_scope)
                            };
                            security_check = Some((ok, message));
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
    if let Some((ok, message)) = security_check {
        checks.push(build_doctor_check(
            "gateway_security_profile",
            ok,
            message,
            "for public binds, configure --token and --tls-cert/--tls-key unless you intentionally opted into the insecure override",
        ));
    }

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
    validate_gateway_security(
        config.bind_addr,
        config.auth_token.as_deref(),
        &config.security,
    )?;
    let listener = TcpListener::bind(config.bind_addr)
        .await
        .map_err(|err| format!("bind failed on {}: {err}", config.bind_addr))?;
    let runtime = KelvinSdkRuntime::initialize(config.runtime)
        .await
        .map_err(|err| err.to_string())?;
    run_gateway_with_listener_secure_and_ingress(
        listener,
        runtime,
        config.auth_token,
        config.security,
        config.ingress,
    )
    .await
}

pub async fn run_gateway_with_listener(
    listener: TcpListener,
    runtime: KelvinSdkRuntime,
    auth_token: Option<String>,
) -> Result<(), String> {
    run_gateway_with_listener_secure_and_ingress(
        listener,
        runtime,
        auth_token,
        GatewaySecurityConfig::default(),
        GatewayIngressConfig::default(),
    )
    .await
}

pub async fn run_gateway_with_listener_secure(
    listener: TcpListener,
    runtime: KelvinSdkRuntime,
    auth_token: Option<String>,
    security: GatewaySecurityConfig,
) -> Result<(), String> {
    run_gateway_with_listener_secure_and_ingress(
        listener,
        runtime,
        auth_token,
        security,
        GatewayIngressConfig::default(),
    )
    .await
}

pub async fn run_gateway_with_listener_secure_and_ingress(
    listener: TcpListener,
    runtime: KelvinSdkRuntime,
    auth_token: Option<String>,
    security: GatewaySecurityConfig,
    ingress: GatewayIngressConfig,
) -> Result<(), String> {
    let local_addr = listener
        .local_addr()
        .map_err(|err| format!("local_addr failed: {err}"))?;
    validate_gateway_security(local_addr, auth_token.as_deref(), &security)?;
    let tls_acceptor = match security.tls.as_ref() {
        Some(config) => Some(load_tls_acceptor(config)?),
        None => None,
    };
    let (ingress_listener, ingress_runtime) = match ingress.bind_listener().await? {
        Some((listener, runtime)) => (Some(listener), Some(runtime)),
        None => (None, None),
    };

    println!(
        "kelvin-gateway listening on {}://{local_addr}",
        gateway_scheme(&security)
    );
    let channel_state_dir = runtime.state_dir().map(Path::to_path_buf);
    let channels = ChannelEngine::from_env_with_state_dir(
        channel_state_dir.as_deref(),
        ingress.channel_exposure(ingress_runtime.as_ref()),
    )
    .map_err(|err| format!("initialize channel engine: {err}"))?;
    let channels = Arc::new(Mutex::new(channels));
    let scheduler = Arc::new(GatewayScheduler::new(runtime.scheduler_store()));
    scheduler.start(runtime.clone(), channels.clone());

    let state = GatewayState {
        bind_addr: local_addr,
        tls_enabled: tls_acceptor.is_some(),
        ingress: ingress_runtime.clone(),
        runtime,
        auth_token: auth_token.map(|value| value.trim().to_string()),
        security: security.clone(),
        started_at: Instant::now(),
        idempotency: Arc::new(Mutex::new(IdempotencyCache::new(2_048))),
        channels,
        scheduler,
        auth_failures: Arc::new(Mutex::new(AuthFailureTracker::new(512))),
        connection_semaphore: Arc::new(Semaphore::new(security.max_connections)),
    };
    if let Some(listener) = ingress_listener {
        ingress::spawn_server(listener, state.clone(), ingress);
    }

    loop {
        let (stream, peer) = listener
            .accept()
            .await
            .map_err(|err| format!("accept failed: {err}"))?;
        let permit = match state.connection_semaphore.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                eprintln!(
                    "gateway connection rejected for {}: max_connections={} reached",
                    peer, state.security.max_connections
                );
                drop(stream);
                continue;
            }
        };
        let connection_state = state.clone();
        let acceptor = tls_acceptor.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let result = match acceptor {
                Some(acceptor) => match acceptor.accept(stream).await {
                    Ok(tls_stream) => {
                        handle_connection(tls_stream, peer.ip(), connection_state).await
                    }
                    Err(err) => Err(format!("tls handshake failed: {err}")),
                },
                None => handle_connection(stream, peer.ip(), connection_state).await,
            };
            if let Err(err) = result {
                eprintln!("gateway connection error for {peer}: {err}");
            }
        });
    }
}

fn gateway_scheme(security: &GatewaySecurityConfig) -> &'static str {
    if security.tls.is_some() {
        "wss"
    } else {
        "ws"
    }
}

fn is_loopback_bind(bind_addr: SocketAddr) -> bool {
    bind_addr.ip().is_loopback()
}

fn validate_gateway_security(
    bind_addr: SocketAddr,
    auth_token: Option<&str>,
    security: &GatewaySecurityConfig,
) -> Result<(), String> {
    if security.max_connections == 0 {
        return Err("gateway max_connections must be >= 1".to_string());
    }
    if security.max_message_size_bytes < 1024 {
        return Err("gateway max_message_size_bytes must be >= 1024".to_string());
    }
    if security.max_frame_size_bytes < 512 {
        return Err("gateway max_frame_size_bytes must be >= 512".to_string());
    }
    if security.max_frame_size_bytes > security.max_message_size_bytes {
        return Err("gateway max_frame_size_bytes must be <= max_message_size_bytes".to_string());
    }
    if security.handshake_timeout_ms < 100 {
        return Err("gateway handshake_timeout_ms must be >= 100".to_string());
    }
    if security.auth_failure_threshold == 0 {
        return Err("gateway auth_failure_threshold must be >= 1".to_string());
    }
    if security.auth_failure_backoff_ms < 100 {
        return Err("gateway auth_failure_backoff_ms must be >= 100".to_string());
    }
    if security.max_outbound_messages_per_connection == 0 {
        return Err("gateway max_outbound_messages_per_connection must be >= 1".to_string());
    }

    let public_bind = !is_loopback_bind(bind_addr);
    let auth_configured = auth_token
        .map(str::trim)
        .map(|token| !token.is_empty())
        .unwrap_or(false);
    if public_bind && !auth_configured {
        return Err(format!(
            "refusing public bind on {} without --token or KELVIN_GATEWAY_TOKEN",
            bind_addr
        ));
    }
    if public_bind && security.tls.is_none() && !security.allow_insecure_public_bind {
        return Err(format!(
            "refusing public bind on {} without TLS; configure --tls-cert/--tls-key or set --allow-insecure-public-bind true for an explicit insecure override",
            bind_addr
        ));
    }
    if let Some(tls) = &security.tls {
        if !tls.cert_path.is_file() {
            return Err(format!(
                "gateway tls cert is missing: {}",
                tls.cert_path.to_string_lossy()
            ));
        }
        if !tls.key_path.is_file() {
            return Err(format!(
                "gateway tls key is missing: {}",
                tls.key_path.to_string_lossy()
            ));
        }
    }

    Ok(())
}

fn load_tls_acceptor(config: &GatewayTlsConfig) -> Result<TlsAcceptor, String> {
    let certs = load_tls_certs(&config.cert_path)?;
    let key = load_tls_key(&config.key_path)?;
    let server_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|err| format!("invalid gateway tls certificate/key pair: {err}"))?;
    Ok(TlsAcceptor::from(Arc::new(server_config)))
}

fn load_tls_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>, String> {
    let file = std::fs::File::open(path)
        .map_err(|err| format!("open gateway tls cert '{}': {err}", path.to_string_lossy()))?;
    let mut reader = BufReader::new(file);
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("read gateway tls cert '{}': {err}", path.to_string_lossy()))
}

fn load_tls_key(path: &Path) -> Result<PrivateKeyDer<'static>, String> {
    let file = std::fs::File::open(path)
        .map_err(|err| format!("open gateway tls key '{}': {err}", path.to_string_lossy()))?;
    let mut reader = BufReader::new(file);
    rustls_pemfile::private_key(&mut reader)
        .map_err(|err| format!("read gateway tls key '{}': {err}", path.to_string_lossy()))?
        .ok_or_else(|| {
            format!(
                "gateway tls key '{}' did not contain a private key",
                path.to_string_lossy()
            )
        })
}

async fn handle_connection<S>(stream: S, peer_ip: IpAddr, state: GatewayState) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let ws_stream = tokio_tungstenite::accept_async_with_config(
        stream,
        Some(tungstenite::protocol::WebSocketConfig {
            max_message_size: Some(state.security.max_message_size_bytes),
            max_frame_size: Some(state.security.max_frame_size_bytes),
            ..Default::default()
        }),
    )
    .await
    .map_err(|err| format!("websocket upgrade failed: {err}"))?;
    let (mut sink, mut source) = ws_stream.split();
    let (writer_tx, mut writer_rx) =
        mpsc::channel::<Message>(state.security.max_outbound_messages_per_connection);

    let writer_task = tokio::spawn(async move {
        while let Some(message) = writer_rx.recv().await {
            if sink.send(message).await.is_err() {
                break;
            }
        }
    });

    if let Some(remaining_ms) = state
        .auth_failures
        .lock()
        .await
        .backoff_remaining_ms(peer_ip)
    {
        let _ = send_error(
            &writer_tx,
            "",
            "unauthorized",
            &format!("auth backoff active; retry after {}ms", remaining_ms),
        );
        let _ = writer_tx.try_send(Message::Close(None));
        drop(writer_tx);
        let _ = writer_task.await;
        return Ok(());
    }

    let first_message = match time::timeout(
        Duration::from_millis(state.security.handshake_timeout_ms),
        source.next(),
    )
    .await
    {
        Err(_) => {
            let _ = send_error(&writer_tx, "", "timeout", "connect handshake timed out");
            let _ = writer_tx.try_send(Message::Close(None));
            drop(writer_tx);
            let _ = writer_task.await;
            return Ok(());
        }
        Ok(Some(Ok(Message::Text(text)))) => text,
        Ok(Some(Ok(_))) => {
            let _ = send_error(
                &writer_tx,
                "",
                "handshake_required",
                "first frame must be a connect request",
            );
            let _ = writer_tx.try_send(Message::Close(None));
            drop(writer_tx);
            let _ = writer_task.await;
            return Ok(());
        }
        Ok(Some(Err(err))) => {
            writer_task.abort();
            return Err(format!("receive failed: {err}"));
        }
        Ok(None) => {
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
        let _ = writer_tx.try_send(Message::Close(None));
        drop(writer_tx);
        let _ = writer_task.await;
        return Ok(());
    }

    let connect_params: ConnectParams = match parse_params(first_params, "connect") {
        Ok(params) => params,
        Err(err) => {
            let _ = send_gateway_error(&writer_tx, &first_id, err);
            let _ = writer_tx.try_send(Message::Close(None));
            drop(writer_tx);
            let _ = writer_task.await;
            return Ok(());
        }
    };
    let _client_id = connect_params
        .client_id
        .unwrap_or_else(|| "unknown".to_string());
    if let Err(err) = verify_auth_token(state.auth_token.as_deref(), connect_params.auth.as_ref()) {
        state
            .auth_failures
            .lock()
            .await
            .record_failure(peer_ip, &state.security);
        let _ = send_gateway_error(&writer_tx, &first_id, err);
        let _ = writer_tx.try_send(Message::Close(None));
        drop(writer_tx);
        let _ = writer_task.await;
        return Ok(());
    }
    state.auth_failures.lock().await.clear(peer_ip);
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
        "health" => {
            let channels = state.channels.lock().await;
            Ok(json!({
                "status": "ok",
                "protocol_version": GATEWAY_PROTOCOL_VERSION,
                "supported_methods": GATEWAY_METHODS_V1,
                "uptime_ms": state.started_at.elapsed().as_millis(),
                "loaded_installed_plugins": state.runtime.loaded_installed_plugins(),
                "security": {
                    "transport": gateway_scheme(&state.security),
                    "bind_addr": state.bind_addr.to_string(),
                    "bind_scope": if is_loopback_bind(state.bind_addr) { "loopback" } else { "public" },
                    "tls_enabled": state.tls_enabled,
                    "auth_required": state.auth_token.is_some(),
                    "allow_insecure_public_bind": state.security.allow_insecure_public_bind,
                    "max_connections": state.security.max_connections,
                    "max_message_size_bytes": state.security.max_message_size_bytes,
                    "max_frame_size_bytes": state.security.max_frame_size_bytes,
                    "handshake_timeout_ms": state.security.handshake_timeout_ms,
                    "auth_failure_threshold": state.security.auth_failure_threshold,
                    "auth_failure_backoff_ms": state.security.auth_failure_backoff_ms,
                    "max_outbound_messages_per_connection": state.security.max_outbound_messages_per_connection,
                    "max_inflight_requests_per_connection": 1,
                },
                "ingress": ingress::GatewayIngressConfig::status_json(state.ingress.as_ref()),
                "channels": {
                    "routing": channels.routing_status(),
                    "telegram": channels.telegram_status(),
                    "slack": channels.slack_status(),
                    "discord": channels.discord_status(),
                },
                "scheduler": state.scheduler.health_payload().await,
            }))
        }
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
        "schedule.list" => {
            let _params: ScheduleListParams = parse_params(params, method)?;
            state.scheduler.list_payload().map_err(map_kelvin_error)
        }
        "schedule.history" => {
            let params: ScheduleHistoryParams = parse_params(params, method)?;
            state
                .scheduler
                .history_payload(params)
                .map_err(map_kelvin_error)
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

fn send_ok(writer_tx: &mpsc::Sender<Message>, id: &str, payload: Value) -> Result<(), String> {
    let frame = ServerFrame::Res {
        id: id.to_string(),
        ok: true,
        payload: Some(payload),
        error: None,
    };
    send_frame(writer_tx, frame)
}

fn send_error(
    writer_tx: &mpsc::Sender<Message>,
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
    writer_tx: &mpsc::Sender<Message>,
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
    writer_tx: &mpsc::Sender<Message>,
    event: &kelvin_core::AgentEvent,
) -> Result<(), String> {
    let payload = serde_json::to_value(event).map_err(|err| err.to_string())?;
    let frame = ServerFrame::Event {
        event: "agent".to_string(),
        payload,
    };
    send_frame(writer_tx, frame)
}

fn send_frame(writer_tx: &mpsc::Sender<Message>, frame: ServerFrame) -> Result<(), String> {
    let text = serde_json::to_string(&frame).map_err(|err| err.to_string())?;
    writer_tx
        .try_send(Message::Text(text))
        .map_err(|_| "connection closed or writer queue full".to_string())
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
                "schedule.history",
                "schedule.list",
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

    #[test]
    fn public_bind_requires_secure_profile_by_default() {
        let bind_addr: SocketAddr = "0.0.0.0:34617".parse().expect("bind addr");
        let error = validate_gateway_security(bind_addr, None, &GatewaySecurityConfig::default())
            .expect_err("public bind should fail closed");
        assert!(
            error.contains("without --token"),
            "unexpected error: {error}"
        );

        let error =
            validate_gateway_security(bind_addr, Some("secret"), &GatewaySecurityConfig::default())
                .expect_err("public ws bind should require tls or override");
        assert!(error.contains("without TLS"), "unexpected error: {error}");
    }

    #[test]
    fn public_bind_can_use_explicit_insecure_override() {
        let bind_addr: SocketAddr = "0.0.0.0:34617".parse().expect("bind addr");
        let security = GatewaySecurityConfig {
            allow_insecure_public_bind: true,
            ..GatewaySecurityConfig::default()
        };
        validate_gateway_security(bind_addr, Some("secret"), &security)
            .expect("explicit insecure override should allow public ws bind");
    }

    #[test]
    fn auth_failure_tracker_enforces_backoff_window() {
        let mut tracker = AuthFailureTracker::new(32);
        let security = GatewaySecurityConfig {
            auth_failure_threshold: 1,
            auth_failure_backoff_ms: 5_000,
            ..GatewaySecurityConfig::default()
        };
        let peer_ip: IpAddr = "127.0.0.1".parse().expect("peer ip");
        tracker.record_failure(peer_ip, &security);
        let remaining = tracker
            .backoff_remaining_ms(peer_ip)
            .expect("backoff should be active");
        assert!(remaining > 0);
        tracker.clear(peer_ip);
        assert!(tracker.backoff_remaining_ms(peer_ip).is_none());
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
