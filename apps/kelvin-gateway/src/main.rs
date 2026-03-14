use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;

use kelvin_core::PluginSecurityPolicy;
use kelvin_gateway::{
    run_gateway, run_gateway_doctor, GatewayConfig, GatewayDoctorConfig, GatewaySecurityConfig,
    GatewayTlsConfig,
};
use kelvin_sdk::{KelvinCliMemoryMode, KelvinSdkModelSelection, KelvinSdkRuntimeConfig};

#[derive(Debug, Clone)]
struct CliConfig {
    bind_addr: SocketAddr,
    auth_token: Option<String>,
    default_session_id: String,
    workspace_dir: PathBuf,
    memory_mode: KelvinCliMemoryMode,
    default_timeout_ms: u64,
    state_dir: Option<PathBuf>,
    persist_runs: bool,
    max_session_history_messages: usize,
    compact_to_messages: usize,
    model_provider: KelvinSdkModelSelection,
    load_installed_plugins: bool,
    require_cli_plugin_tool: bool,
    doctor_mode: bool,
    doctor_endpoint: String,
    doctor_plugin_home: PathBuf,
    doctor_trust_policy_path: PathBuf,
    doctor_timeout_ms: u64,
    security: GatewaySecurityConfig,
}

fn usage() -> &'static str {
    "Usage: kelvin-gateway [--bind <host:port>] [--token <token>] [--tls-cert <path>] [--tls-key <path>] [--allow-insecure-public-bind true|false] [--max-connections <n>] [--max-message-bytes <n>] [--max-frame-bytes <n>] [--handshake-timeout-ms <ms>] [--auth-failure-threshold <n>] [--auth-failure-backoff-ms <ms>] [--max-outbound-messages <n>] [--session <id>] [--workspace <dir>] [--memory markdown|in-memory|fallback] [--timeout-ms <ms>] [--state-dir <path>] [--persist-runs true|false] [--max-session-history <n>] [--compact-to <n>] [--model-provider <plugin_id>] [--model-provider-failover <id1,id2,...>] [--failover-retries <n>] [--failover-backoff-ms <ms>] [--load-installed-plugins true|false] [--require-cli-plugin true|false] [--doctor] [--endpoint <ws://host:port>] [--plugin-home <path>] [--trust-policy <path>] [--doctor-timeout-ms <ms>]"
}

fn parse_bool(value: &str, flag: &str) -> Result<bool, String> {
    let normalized = value.trim().to_lowercase();
    match normalized.as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(format!("invalid boolean value for {flag}: {value}")),
    }
}

fn parse_u64(value: &str, flag: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| format!("invalid numeric value for {flag}"))
}

fn parse_u32(value: &str, flag: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .map_err(|_| format!("invalid numeric value for {flag}"))
}

fn parse_usize(value: &str, flag: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| format!("invalid numeric value for {flag}"))
}

fn env_bool(name: &str, default: bool) -> Result<bool, String> {
    match env::var(name) {
        Ok(value) => parse_bool(&value, name),
        Err(_) => Ok(default),
    }
}

fn env_u64(name: &str, default: u64) -> Result<u64, String> {
    match env::var(name) {
        Ok(value) => parse_u64(&value, name),
        Err(_) => Ok(default),
    }
}

fn env_u32(name: &str, default: u32) -> Result<u32, String> {
    match env::var(name) {
        Ok(value) => parse_u32(&value, name),
        Err(_) => Ok(default),
    }
}

fn env_usize(name: &str, default: usize) -> Result<usize, String> {
    match env::var(name) {
        Ok(value) => parse_usize(&value, name),
        Err(_) => Ok(default),
    }
}

fn env_optional_path(name: &str) -> Option<PathBuf> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn parse_args() -> Result<CliConfig, String> {
    let mut bind_addr: SocketAddr = "127.0.0.1:34617"
        .parse()
        .map_err(|err| format!("invalid default bind addr: {err}"))?;
    let mut auth_token = env::var("KELVIN_GATEWAY_TOKEN").ok();
    let mut default_session_id = "main".to_string();
    let mut workspace_dir = env::current_dir().map_err(|err| err.to_string())?;
    let mut memory_mode = KelvinCliMemoryMode::Markdown;
    let mut default_timeout_ms = 30_000_u64;
    let mut state_dir: Option<PathBuf> = None;
    let mut persist_runs = true;
    let mut max_session_history_messages = 128_usize;
    let mut compact_to_messages = 64_usize;
    let mut model_provider = KelvinSdkModelSelection::Echo;
    let mut load_installed_plugins = true;
    let mut require_cli_plugin_tool = false;
    let mut doctor_mode = false;
    let mut doctor_endpoint = "ws://127.0.0.1:34617".to_string();
    let mut doctor_timeout_ms = 5_000_u64;
    let mut doctor_plugin_home = PathBuf::from(".kelvin/plugins");
    let mut doctor_trust_policy_path = PathBuf::from(".kelvin/trusted_publishers.json");
    let mut failover_retries = 1_u8;
    let mut failover_backoff_ms = 100_u64;
    let mut pending_failover_ids: Option<Vec<String>> = None;
    let mut allow_insecure_public_bind =
        env_bool("KELVIN_GATEWAY_ALLOW_INSECURE_PUBLIC_BIND", false)?;
    let mut tls_cert_path = env_optional_path("KELVIN_GATEWAY_TLS_CERT_PATH");
    let mut tls_key_path = env_optional_path("KELVIN_GATEWAY_TLS_KEY_PATH");
    let mut max_connections = env_usize("KELVIN_GATEWAY_MAX_CONNECTIONS", 128)?;
    let mut max_message_size_bytes = env_usize("KELVIN_GATEWAY_MAX_MESSAGE_BYTES", 64 * 1024)?;
    let mut max_frame_size_bytes = env_usize("KELVIN_GATEWAY_MAX_FRAME_BYTES", 16 * 1024)?;
    let mut handshake_timeout_ms = env_u64("KELVIN_GATEWAY_HANDSHAKE_TIMEOUT_MS", 5_000)?;
    let mut auth_failure_threshold = env_u32("KELVIN_GATEWAY_AUTH_FAILURE_THRESHOLD", 3)?;
    let mut auth_failure_backoff_ms =
        env_u64("KELVIN_GATEWAY_AUTH_FAILURE_BACKOFF_MS", 1_500)?;
    let mut max_outbound_messages_per_connection =
        env_usize("KELVIN_GATEWAY_MAX_OUTBOUND_MESSAGES", 128)?;

    let mut args = env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => return Err(usage().to_string()),
            "--bind" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --bind".to_string())?;
                bind_addr = value
                    .parse::<SocketAddr>()
                    .map_err(|err| format!("invalid --bind value '{value}': {err}"))?;
            }
            "--doctor" => {
                doctor_mode = true;
            }
            "--endpoint" => {
                doctor_endpoint = args
                    .next()
                    .ok_or_else(|| "missing value for --endpoint".to_string())?;
            }
            "--plugin-home" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --plugin-home".to_string())?;
                doctor_plugin_home = PathBuf::from(value);
            }
            "--trust-policy" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --trust-policy".to_string())?;
                doctor_trust_policy_path = PathBuf::from(value);
            }
            "--doctor-timeout-ms" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --doctor-timeout-ms".to_string())?;
                doctor_timeout_ms = value
                    .parse::<u64>()
                    .map_err(|_| "invalid numeric value for --doctor-timeout-ms".to_string())?;
            }
            "--token" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --token".to_string())?;
                let trimmed = value.trim();
                auth_token = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                };
            }
            "--tls-cert" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --tls-cert".to_string())?;
                tls_cert_path = Some(PathBuf::from(value));
            }
            "--tls-key" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --tls-key".to_string())?;
                tls_key_path = Some(PathBuf::from(value));
            }
            "--allow-insecure-public-bind" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --allow-insecure-public-bind".to_string())?;
                allow_insecure_public_bind =
                    parse_bool(&value, "--allow-insecure-public-bind")?;
            }
            "--max-connections" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --max-connections".to_string())?;
                max_connections = parse_usize(&value, "--max-connections")?;
            }
            "--max-message-bytes" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --max-message-bytes".to_string())?;
                max_message_size_bytes = parse_usize(&value, "--max-message-bytes")?;
            }
            "--max-frame-bytes" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --max-frame-bytes".to_string())?;
                max_frame_size_bytes = parse_usize(&value, "--max-frame-bytes")?;
            }
            "--handshake-timeout-ms" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --handshake-timeout-ms".to_string())?;
                handshake_timeout_ms = parse_u64(&value, "--handshake-timeout-ms")?;
            }
            "--auth-failure-threshold" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --auth-failure-threshold".to_string())?;
                auth_failure_threshold = parse_u32(&value, "--auth-failure-threshold")?;
            }
            "--auth-failure-backoff-ms" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --auth-failure-backoff-ms".to_string())?;
                auth_failure_backoff_ms =
                    parse_u64(&value, "--auth-failure-backoff-ms")?;
            }
            "--max-outbound-messages" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --max-outbound-messages".to_string())?;
                max_outbound_messages_per_connection =
                    parse_usize(&value, "--max-outbound-messages")?;
            }
            "--session" => {
                default_session_id = args
                    .next()
                    .ok_or_else(|| "missing value for --session".to_string())?;
            }
            "--workspace" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --workspace".to_string())?;
                workspace_dir = PathBuf::from(value);
            }
            "--memory" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --memory".to_string())?;
                memory_mode = KelvinCliMemoryMode::parse(&value);
            }
            "--timeout-ms" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --timeout-ms".to_string())?;
                default_timeout_ms = value
                    .parse::<u64>()
                    .map_err(|_| "invalid numeric value for --timeout-ms".to_string())?;
            }
            "--state-dir" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --state-dir".to_string())?;
                state_dir = Some(PathBuf::from(value));
            }
            "--persist-runs" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --persist-runs".to_string())?;
                persist_runs = parse_bool(&value, "--persist-runs")?;
            }
            "--max-session-history" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --max-session-history".to_string())?;
                max_session_history_messages = value
                    .parse::<usize>()
                    .map_err(|_| "invalid numeric value for --max-session-history".to_string())?;
            }
            "--compact-to" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --compact-to".to_string())?;
                compact_to_messages = value
                    .parse::<usize>()
                    .map_err(|_| "invalid numeric value for --compact-to".to_string())?;
            }
            "--model-provider" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --model-provider".to_string())?;
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    return Err("model provider id must not be empty".to_string());
                }
                model_provider = KelvinSdkModelSelection::InstalledPlugin {
                    plugin_id: trimmed.to_string(),
                };
            }
            "--model-provider-failover" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --model-provider-failover".to_string())?;
                let ids = value
                    .split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(|item| item.to_string())
                    .collect::<Vec<_>>();
                if ids.is_empty() {
                    return Err("model provider failover list must not be empty".to_string());
                }
                pending_failover_ids = Some(ids);
            }
            "--failover-retries" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --failover-retries".to_string())?;
                failover_retries = value
                    .parse::<u8>()
                    .map_err(|_| "invalid numeric value for --failover-retries".to_string())?;
            }
            "--failover-backoff-ms" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --failover-backoff-ms".to_string())?;
                failover_backoff_ms = value
                    .parse::<u64>()
                    .map_err(|_| "invalid numeric value for --failover-backoff-ms".to_string())?;
            }
            "--load-installed-plugins" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --load-installed-plugins".to_string())?;
                load_installed_plugins = parse_bool(&value, "--load-installed-plugins")?;
            }
            "--require-cli-plugin" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --require-cli-plugin".to_string())?;
                require_cli_plugin_tool = parse_bool(&value, "--require-cli-plugin")?;
            }
            unknown => return Err(format!("unknown argument: {unknown}\n{}", usage())),
        }
    }

    if let Some(ids) = pending_failover_ids {
        model_provider = KelvinSdkModelSelection::InstalledPluginFailover {
            plugin_ids: ids,
            max_retries_per_provider: failover_retries,
            retry_backoff_ms: failover_backoff_ms,
        };
    }

    let tls = match (tls_cert_path, tls_key_path) {
        (None, None) => None,
        (Some(cert_path), Some(key_path)) => Some(GatewayTlsConfig {
            cert_path,
            key_path,
        }),
        (Some(_), None) => {
            return Err("gateway TLS requires both certificate and key paths".to_string())
        }
        (None, Some(_)) => {
            return Err("gateway TLS requires both certificate and key paths".to_string())
        }
    };

    Ok(CliConfig {
        bind_addr,
        auth_token,
        default_session_id,
        workspace_dir,
        memory_mode,
        default_timeout_ms,
        state_dir,
        persist_runs,
        max_session_history_messages,
        compact_to_messages,
        model_provider,
        load_installed_plugins,
        require_cli_plugin_tool,
        doctor_mode,
        doctor_endpoint,
        doctor_plugin_home,
        doctor_trust_policy_path,
        doctor_timeout_ms,
        security: GatewaySecurityConfig {
            tls,
            allow_insecure_public_bind,
            max_connections,
            max_message_size_bytes,
            max_frame_size_bytes,
            handshake_timeout_ms,
            auth_failure_threshold,
            auth_failure_backoff_ms,
            max_outbound_messages_per_connection,
        },
    })
}

fn selection_requires_network(policy: &KelvinSdkModelSelection) -> bool {
    !matches!(policy, KelvinSdkModelSelection::Echo)
}

#[tokio::main]
async fn main() {
    match parse_args() {
        Ok(config) => {
            if config.doctor_mode {
                let report = run_gateway_doctor(GatewayDoctorConfig {
                    endpoint: config.doctor_endpoint,
                    auth_token: config.auth_token,
                    plugin_home: config.doctor_plugin_home,
                    trust_policy_path: config.doctor_trust_policy_path,
                    timeout_ms: config.doctor_timeout_ms,
                })
                .await;
                match report {
                    Ok(value) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&value)
                                .unwrap_or_else(|_| value.to_string())
                        );
                        if !value.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                            std::process::exit(1);
                        }
                    }
                    Err(err) => {
                        eprintln!("doctor error: {err}");
                        std::process::exit(1);
                    }
                }
                return;
            }

            let mut plugin_security_policy = PluginSecurityPolicy::default();
            if selection_requires_network(&config.model_provider) {
                plugin_security_policy.allow_network_egress = true;
            }

            let state_dir = config.workspace_dir.join(".kelvin").join("state");
            let runtime_config = KelvinSdkRuntimeConfig {
                workspace_dir: config.workspace_dir,
                default_session_id: config.default_session_id,
                memory_mode: config.memory_mode,
                default_timeout_ms: config.default_timeout_ms,
                default_system_prompt: None,
                core_version: "0.1.0".to_string(),
                plugin_security_policy,
                load_installed_plugins: config.load_installed_plugins,
                model_provider: config.model_provider,
                require_cli_plugin_tool: config.require_cli_plugin_tool,
                emit_stdout_events: false,
                state_dir: config.state_dir.or(Some(state_dir)),
                persist_runs: config.persist_runs,
                max_session_history_messages: config.max_session_history_messages,
                compact_to_messages: config.compact_to_messages,
            };
            let gateway_config = GatewayConfig {
                bind_addr: config.bind_addr,
                auth_token: config.auth_token,
                runtime: runtime_config,
                security: config.security,
            };
            if let Err(err) = run_gateway(gateway_config).await {
                eprintln!("gateway error: {err}");
                std::process::exit(1);
            }
        }
        Err(err) => {
            eprintln!("{err}");
            if err.starts_with("Usage:") {
                std::process::exit(0);
            }
            std::process::exit(1);
        }
    }
}
