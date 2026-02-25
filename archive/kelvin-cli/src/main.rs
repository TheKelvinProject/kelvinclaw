use std::env;
use std::path::PathBuf;

use kelvin_sdk::{run_with_sdk, KelvinCliMemoryMode, KelvinSdkConfig};

#[derive(Debug, Clone)]
struct CliConfig {
    prompt: String,
    session_id: String,
    workspace_dir: PathBuf,
    memory_mode: KelvinCliMemoryMode,
    timeout_ms: u64,
    system_prompt: Option<String>,
}

fn usage() -> &'static str {
    "Usage: kelvin-cli --prompt <text> [--session <id>] [--workspace <dir>] [--memory markdown|in-memory|fallback] [--timeout-ms <ms>]"
}

fn parse_args() -> Result<CliConfig, String> {
    let mut prompt: Option<String> = None;
    let mut session_id = "main".to_string();
    let mut workspace_dir = env::current_dir().map_err(|err| err.to_string())?;
    let mut memory_mode = KelvinCliMemoryMode::Markdown;
    let mut timeout_ms = 30_000_u64;
    let mut system_prompt: Option<String> = None;

    let mut args = env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => return Err(usage().to_string()),
            "--prompt" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --prompt".to_string())?;
                prompt = Some(value);
            }
            "--session" => {
                session_id = args
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
                timeout_ms = value
                    .parse::<u64>()
                    .map_err(|_| "invalid numeric value for --timeout-ms".to_string())?;
            }
            "--system" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --system".to_string())?;
                system_prompt = Some(value);
            }
            other if !other.starts_with('-') && prompt.is_none() => {
                prompt = Some(other.to_string());
            }
            unknown => {
                return Err(format!("unknown argument: {unknown}\n{}", usage()));
            }
        }
    }

    let prompt = prompt.ok_or_else(|| format!("missing prompt\n{}", usage()))?;

    Ok(CliConfig {
        prompt,
        session_id,
        workspace_dir,
        memory_mode,
        timeout_ms,
        system_prompt,
    })
}

#[tokio::main]
async fn main() {
    match parse_args() {
        Ok(config) => {
            let result = run_with_sdk(KelvinSdkConfig {
                prompt: config.prompt,
                session_id: config.session_id,
                workspace_dir: config.workspace_dir,
                memory_mode: config.memory_mode,
                timeout_ms: config.timeout_ms,
                system_prompt: config.system_prompt,
                core_version: "0.1.0".to_string(),
                plugin_security_policy: Default::default(),
                load_installed_plugins: true,
            })
            .await;

            match result {
                Ok(summary) => {
                    println!("cli plugin preflight: {}", summary.cli_plugin_preflight);
                    println!(
                        "run complete in {}ms (provider={}, model={})",
                        summary.duration_ms, summary.provider, summary.model
                    );
                    for payload in summary.payloads {
                        println!("payload: {payload}");
                    }
                }
                Err(err) => {
                    eprintln!("error: {err}");
                    std::process::exit(1);
                }
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
