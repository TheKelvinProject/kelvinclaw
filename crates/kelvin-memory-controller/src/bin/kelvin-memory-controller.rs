use std::fs;
use std::net::SocketAddr;

use tonic::transport::{Server, ServerTlsConfig};

use kelvin_memory_api::v1alpha1::memory_service_server::MemoryServiceServer;
use kelvin_memory_api::MemoryModuleManifest;
use kelvin_memory_controller::{MemoryController, MemoryControllerConfig, ProviderRegistry};

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = std::env::var("KELVIN_MEMORY_CONTROLLER_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:50051".to_string())
        .parse()?;
    let allow_insecure_non_loopback = std::env::var("KELVIN_MEMORY_ALLOW_INSECURE_NON_LOOPBACK")
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            normalized == "1" || normalized == "true" || normalized == "yes"
        })
        .unwrap_or(false);

    let cfg = MemoryControllerConfig::from_env();
    if cfg.decoding_key_pem.trim().is_empty() && cfg.decoding_key_path.trim().is_empty() {
        return Err(
            "KELVIN_MEMORY_PUBLIC_KEY_PEM or KELVIN_MEMORY_PUBLIC_KEY_PATH is required".into(),
        );
    }
    let tls_identity = cfg.tls_identity()?;
    let tls_client_ca = cfg.tls_client_ca()?;
    let tls_enabled = tls_identity.is_some();

    if !addr.ip().is_loopback() && !tls_enabled && !allow_insecure_non_loopback {
        return Err("refusing insecure non-loopback bind without TLS. \
set KELVIN_MEMORY_TLS_CERT_PATH/KELVIN_MEMORY_TLS_KEY_PATH for TLS, or \
set KELVIN_MEMORY_ALLOW_INSECURE_NON_LOOPBACK=true only behind a trusted network boundary"
            .into());
    }

    if tls_client_ca.is_some() && !tls_enabled {
        return Err("mTLS client CA configured but server TLS cert/key is missing".into());
    }

    let controller = MemoryController::new(cfg, ProviderRegistry::with_default_in_memory())?;

    if let Ok(manifest_path) = std::env::var("KELVIN_MEMORY_MODULE_MANIFEST") {
        let manifest_bytes = fs::read(&manifest_path)?;
        let manifest: MemoryModuleManifest = serde_json::from_slice(&manifest_bytes)?;
        let wasm_bytes = if let Ok(wasm_path) = std::env::var("KELVIN_MEMORY_MODULE_WASM") {
            fs::read(&wasm_path)?
        } else if let Ok(wat_path) = std::env::var("KELVIN_MEMORY_MODULE_WAT") {
            wat::parse_file(&wat_path)?
        } else {
            return Err(
                "KELVIN_MEMORY_MODULE_MANIFEST requires KELVIN_MEMORY_MODULE_WASM or KELVIN_MEMORY_MODULE_WAT"
                    .into(),
            );
        };
        controller
            .register_module_bytes(manifest, &wasm_bytes)
            .await?;
    }

    let mut server = Server::builder();
    if let Some(identity) = tls_identity {
        let mut tls = ServerTlsConfig::new().identity(identity);
        if let Some(client_ca) = tls_client_ca {
            tls = tls.client_ca_root(client_ca).client_auth_optional(false);
            println!("kelvin-memory-controller listening on {addr} (TLS+mTLS)");
        } else {
            println!("kelvin-memory-controller listening on {addr} (TLS)");
        }
        server = server.tls_config(tls)?;
    } else {
        println!("kelvin-memory-controller listening on {addr} (plaintext)");
    }

    server
        .add_service(MemoryServiceServer::new(controller))
        .serve(addr)
        .await?;
    Ok(())
}
