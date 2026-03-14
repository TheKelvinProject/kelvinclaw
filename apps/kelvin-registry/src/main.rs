use std::net::SocketAddr;
use std::path::PathBuf;

use kelvin_registry::{run_registry, RegistryConfig};

fn usage() -> &'static str {
    "Usage: kelvin-registry --index <path> [--bind <host:port>] [--trust-policy <path>]"
}

fn parse_args() -> Result<RegistryConfig, String> {
    let mut bind_addr = std::env::var("KELVIN_PLUGIN_REGISTRY_BIND")
        .ok()
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("127.0.0.1:34718")
        .parse::<SocketAddr>()
        .map_err(|err| format!("invalid KELVIN_PLUGIN_REGISTRY_BIND value: {err}"))?;
    let mut index_path = std::env::var("KELVIN_PLUGIN_REGISTRY_INDEX")
        .ok()
        .map(PathBuf::from);
    let mut trust_policy_path = std::env::var("KELVIN_PLUGIN_REGISTRY_TRUST_POLICY")
        .ok()
        .map(PathBuf::from);

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bind" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --bind".to_string())?;
                bind_addr = value
                    .parse::<SocketAddr>()
                    .map_err(|err| format!("invalid --bind value '{value}': {err}"))?;
            }
            "--index" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --index".to_string())?;
                index_path = Some(PathBuf::from(value));
            }
            "--trust-policy" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --trust-policy".to_string())?;
                trust_policy_path = Some(PathBuf::from(value));
            }
            "-h" | "--help" => {
                println!("{}", usage());
                std::process::exit(0);
            }
            _ => {
                return Err(format!("unknown argument: {arg}\n{}", usage()));
            }
        }
    }

    let index_path = index_path.ok_or_else(|| {
        format!(
            "missing registry index path; set --index <path> or KELVIN_PLUGIN_REGISTRY_INDEX\n{}",
            usage()
        )
    })?;
    Ok(RegistryConfig {
        bind_addr,
        index_path,
        trust_policy_path,
    })
}

#[tokio::main]
async fn main() {
    match parse_args() {
        Ok(config) => {
            if let Err(err) = run_registry(config).await {
                eprintln!("registry error: {err}");
                std::process::exit(1);
            }
        }
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}
