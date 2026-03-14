use std::net::SocketAddr;
use std::sync::OnceLock;

use rcgen::generate_simple_self_signed;
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};

use kelvin_core::{MemoryReadParams, MemorySearchManager};
use kelvin_memory_api::v1alpha1::memory_service_server::MemoryServiceServer;
use kelvin_memory_api::MemoryModuleManifest;
use kelvin_memory_client::{MemoryClientConfig, RpcMemoryManager};
use kelvin_memory_controller::{MemoryController, MemoryControllerConfig, ProviderRegistry};

const TEST_PRIVATE_KEY_DER_B64: &str =
    "MC4CAQAwBQYDK2VwBCIEIHCRmiDXsIoP30rbpS6V729OHS4HzRnpgTwSC9zqETba";
const TEST_PUBLIC_KEY_DER_B64: &str =
    "MCowBQYDK2VwAyEAHOzip8DiPZOcMhc+e66Wzd1ifXEFAP8DEGUzJFg/DBc=";

fn test_private_key_pem() -> String {
    format!(
        "-----{} PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
        "BEGIN", TEST_PRIVATE_KEY_DER_B64
    )
}

fn test_public_key_pem() -> String {
    format!(
        "-----{} PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
        "BEGIN", TEST_PUBLIC_KEY_DER_B64
    )
}

fn sample_manifest() -> MemoryModuleManifest {
    MemoryModuleManifest {
        module_id: "memory.echo".to_string(),
        version: "0.1.0".to_string(),
        api_version: "0.1.0".to_string(),
        capabilities: vec![
            "memory_crud".to_string(),
            "memory_read".to_string(),
            "memory_health".to_string(),
        ],
        required_host_features: vec!["provider_sqlite".to_string()],
        entrypoint: "memory_echo.wasm".to_string(),
        publisher: "acme".to_string(),
        signature: "test-signature".to_string(),
    }
}

fn sample_wasm() -> Vec<u8> {
    wat::parse_str(
        r#"
        (module
          (import "memory_host" "kv_get" (func $kv_get (param i32) (result i32)))
          (import "memory_host" "kv_put" (func $kv_put (param i32) (result i32)))
          (import "memory_host" "blob_get" (func $blob_get (param i32) (result i32)))
          (import "memory_host" "blob_put" (func $blob_put (param i32) (result i32)))
          (import "memory_host" "emit_metric" (func $emit_metric (param i32) (result i32)))
          (import "memory_host" "log" (func $log (param i32) (result i32)))
          (import "memory_host" "clock_now_ms" (func $clock (result i64)))
          (func (export "handle_upsert") (result i32) i32.const 0)
          (func (export "handle_query") (result i32) i32.const 0)
          (func (export "handle_read") (result i32) i32.const 0)
          (func (export "handle_delete") (result i32) i32.const 0)
          (func (export "handle_health") (result i32) i32.const 0)
        )
        "#,
    )
    .expect("compile wat")
}

struct TestTlsMaterial {
    ca_pem: String,
    server_cert_pem: String,
    server_key_pem: String,
    client_cert_pem: String,
    client_key_pem: String,
}

fn generate_test_tls_material() -> TestTlsMaterial {
    let server = generate_simple_self_signed(vec!["localhost".to_string()]).expect("server cert");
    let client = generate_simple_self_signed(vec!["localhost".to_string()]).expect("client cert");

    TestTlsMaterial {
        ca_pem: client.cert.pem(),
        server_cert_pem: server.cert.pem(),
        server_key_pem: server.signing_key.serialize_pem(),
        client_cert_pem: client.cert.pem(),
        client_key_pem: client.signing_key.serialize_pem(),
    }
}

fn ensure_rustls_crypto_provider() {
    static RUSTLS_PROVIDER: OnceLock<()> = OnceLock::new();
    let _ = RUSTLS_PROVIDER.get_or_init(|| {
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    });
}

async fn start_test_server() -> SocketAddr {
    start_test_server_with_tls(None).await
}

async fn start_test_server_with_tls(tls: Option<TestTlsMaterial>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("local addr");

    let mut cfg = MemoryControllerConfig::default();
    cfg.decoding_key_pem = test_public_key_pem();
    let controller =
        MemoryController::new(cfg, ProviderRegistry::with_default_in_memory()).expect("controller");
    controller
        .register_module_bytes(sample_manifest(), &sample_wasm())
        .await
        .expect("register module");

    tokio::spawn(async move {
        let mut builder = Server::builder();
        if let Some(tls) = tls {
            ensure_rustls_crypto_provider();
            let mut tls_cfg = ServerTlsConfig::new()
                .identity(Identity::from_pem(tls.server_cert_pem, tls.server_key_pem));
            if !tls.ca_pem.trim().is_empty() {
                tls_cfg = tls_cfg
                    .client_ca_root(Certificate::from_pem(tls.ca_pem))
                    .client_auth_optional(false);
            }
            builder = builder.tls_config(tls_cfg).expect("server tls config");
        }
        builder
            .add_service(MemoryServiceServer::new(controller))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .expect("serve");
    });

    addr
}

#[tokio::test]
async fn rpc_memory_manager_crud_and_search_roundtrip() {
    let addr = start_test_server().await;
    let cfg = MemoryClientConfig {
        endpoint: format!("http://{addr}"),
        signing_key_pem: test_private_key_pem(),
        tenant_id: "tenant-a".to_string(),
        workspace_id: "workspace-a".to_string(),
        session_id: "session-a".to_string(),
        module_id: "memory.echo".to_string(),
        ..Default::default()
    };
    let manager = RpcMemoryManager::connect(cfg).await.expect("connect");

    manager
        .upsert("MEMORY.md", b"configured router on vlan10")
        .await
        .expect("upsert");

    let hits = manager
        .search("router", Default::default())
        .await
        .expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "MEMORY.md");

    let read = manager
        .read_file(MemoryReadParams {
            rel_path: "MEMORY.md".to_string(),
            from: None,
            lines: None,
        })
        .await
        .expect("read");
    assert!(read.text.contains("vlan10"));
}

#[tokio::test]
async fn rpc_memory_manager_unavailable_returns_typed_backend_error() {
    let cfg = MemoryClientConfig {
        endpoint: "http://127.0.0.1:65534".to_string(),
        signing_key_pem: test_private_key_pem(),
        ..Default::default()
    };
    let err = match RpcMemoryManager::connect(cfg).await {
        Ok(_) => panic!("connect should fail"),
        Err(err) => err,
    };
    assert!(err
        .to_string()
        .to_lowercase()
        .contains("memory controller unavailable"));
}

#[tokio::test]
async fn rpc_memory_manager_tls_roundtrip() {
    let tls = generate_test_tls_material();
    let addr = start_test_server_with_tls(Some(TestTlsMaterial {
        ca_pem: String::new(),
        server_cert_pem: tls.server_cert_pem.clone(),
        server_key_pem: tls.server_key_pem.clone(),
        client_cert_pem: tls.client_cert_pem.clone(),
        client_key_pem: tls.client_key_pem.clone(),
    }))
    .await;
    let cfg = MemoryClientConfig {
        endpoint: format!("https://localhost:{}", addr.port()),
        signing_key_pem: test_private_key_pem(),
        tls_ca_pem: tls.server_cert_pem.clone(),
        tls_domain_name: "localhost".to_string(),
        tenant_id: "tenant-tls".to_string(),
        workspace_id: "workspace-tls".to_string(),
        session_id: "session-tls".to_string(),
        module_id: "memory.echo".to_string(),
        ..Default::default()
    };
    let manager = RpcMemoryManager::connect(cfg).await.expect("connect tls");
    manager
        .upsert("TLS.md", b"secure transport enabled")
        .await
        .expect("upsert tls");
    let hits = manager
        .search("secure", Default::default())
        .await
        .expect("search tls");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "TLS.md");
}

#[tokio::test]
async fn rpc_memory_manager_mtls_missing_client_identity_fails() {
    let tls = generate_test_tls_material();
    let addr = start_test_server_with_tls(Some(TestTlsMaterial {
        ca_pem: tls.ca_pem.clone(),
        server_cert_pem: tls.server_cert_pem.clone(),
        server_key_pem: tls.server_key_pem.clone(),
        client_cert_pem: tls.client_cert_pem.clone(),
        client_key_pem: tls.client_key_pem.clone(),
    }))
    .await;
    let cfg = MemoryClientConfig {
        endpoint: format!("https://localhost:{}", addr.port()),
        signing_key_pem: test_private_key_pem(),
        tls_ca_pem: tls.server_cert_pem,
        tls_domain_name: "localhost".to_string(),
        ..Default::default()
    };
    let manager = RpcMemoryManager::connect(cfg)
        .await
        .expect("channel initialization can be lazy");
    let err = match manager.upsert("MTLS_FAIL.md", b"should fail").await {
        Ok(_) => panic!("request should fail without mTLS client identity"),
        Err(err) => err,
    };
    assert!(err
        .to_string()
        .to_lowercase()
        .contains("memory controller unavailable"));
}

#[tokio::test]
async fn rpc_memory_manager_mtls_roundtrip_with_client_identity() {
    let tls = generate_test_tls_material();
    let addr = start_test_server_with_tls(Some(TestTlsMaterial {
        ca_pem: tls.ca_pem.clone(),
        server_cert_pem: tls.server_cert_pem.clone(),
        server_key_pem: tls.server_key_pem.clone(),
        client_cert_pem: tls.client_cert_pem.clone(),
        client_key_pem: tls.client_key_pem.clone(),
    }))
    .await;
    let cfg = MemoryClientConfig {
        endpoint: format!("https://localhost:{}", addr.port()),
        signing_key_pem: test_private_key_pem(),
        tls_ca_pem: tls.server_cert_pem.clone(),
        tls_domain_name: "localhost".to_string(),
        tls_client_cert_pem: tls.client_cert_pem,
        tls_client_key_pem: tls.client_key_pem,
        tenant_id: "tenant-mtls".to_string(),
        workspace_id: "workspace-mtls".to_string(),
        session_id: "session-mtls".to_string(),
        module_id: "memory.echo".to_string(),
        ..Default::default()
    };
    let manager = RpcMemoryManager::connect(cfg)
        .await
        .expect("connect with mTLS");
    manager
        .upsert("MTLS.md", b"mutual tls enabled")
        .await
        .expect("upsert mtls");
    let hits = manager
        .search("mutual", Default::default())
        .await
        .expect("search mtls");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "MTLS.md");
}
