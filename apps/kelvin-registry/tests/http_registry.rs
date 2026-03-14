use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use kelvin_registry::{router, RegistryConfig};
use reqwest::Client;
use serde_json::json;
use tokio::net::TcpListener;

fn unique_root() -> std::path::PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let root = std::env::temp_dir().join(format!("kelvin-registry-test-{millis}"));
    fs::create_dir_all(&root).expect("create registry test root");
    root
}

async fn start_registry() -> (String, tokio::task::JoinHandle<()>) {
    let root = unique_root();
    let index_path = root.join("index.json");
    let trust_policy_path = root.join("trusted_publishers.json");
    fs::write(
        &index_path,
        serde_json::to_vec_pretty(&json!({
            "schema_version": "v1",
            "plugins": [
                {
                    "id": "kelvin.cli",
                    "version": "0.10.0",
                    "package_url": "https://example.com/cli-0.10.0.tar.gz",
                    "sha256": "b".repeat(64),
                    "quality_tier": "signed_trusted",
                    "tags": ["cli", "first_party"]
                },
                {
                    "id": "kelvin.cli",
                    "version": "0.1.0",
                    "package_url": "https://example.com/cli-0.1.0.tar.gz",
                    "sha256": "a".repeat(64),
                    "quality_tier": "signed_trusted",
                    "tags": ["cli"]
                },
                {
                    "id": "kelvin.openai",
                    "version": "0.1.0",
                    "package_url": "https://example.com/openai-0.1.0.tar.gz",
                    "sha256": "c".repeat(64),
                    "trust_policy_url": "https://example.com/trusted_publishers.json",
                    "quality_tier": "signed_trusted",
                    "tags": ["model", "first_party"]
                }
            ]
        }))
        .expect("serialize registry index"),
    )
    .expect("write registry index");
    fs::write(
        &trust_policy_path,
        serde_json::to_vec_pretty(&json!({
            "require_signature": true,
            "publishers": [{"id": "kelvin", "ed25519_public_key": "test"}]
        }))
        .expect("serialize trust policy"),
    )
    .expect("write trust policy");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("local addr");
    let app = router(RegistryConfig {
        bind_addr: addr,
        index_path,
        trust_policy_path: Some(trust_policy_path),
    })
    .expect("build registry router");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), handle)
}

#[tokio::test]
async fn registry_serves_health_and_filtered_plugin_views() {
    let (root, handle) = start_registry().await;
    let client = Client::new();

    let health: serde_json::Value = client
        .get(format!("{root}/healthz"))
        .send()
        .await
        .expect("health request")
        .json()
        .await
        .expect("health payload");
    assert_eq!(health["ok"], json!(true));
    assert_eq!(health["plugin_count"], json!(3));

    let plugins: serde_json::Value = client
        .get(format!("{root}/v1/plugins?latest_only=true"))
        .send()
        .await
        .expect("plugins request")
        .json()
        .await
        .expect("plugins payload");
    assert_eq!(plugins["count"], json!(2));
    assert_eq!(plugins["plugins"][0]["id"], json!("kelvin.cli"));
    assert_eq!(plugins["plugins"][0]["version"], json!("0.10.0"));

    let filtered: serde_json::Value = client
        .get(format!("{root}/v1/plugins?tag=model"))
        .send()
        .await
        .expect("filtered request")
        .json()
        .await
        .expect("filtered payload");
    assert_eq!(filtered["count"], json!(1));
    assert_eq!(filtered["plugins"][0]["id"], json!("kelvin.openai"));

    handle.abort();
}

#[tokio::test]
async fn registry_serves_plugin_versions_and_trust_policy() {
    let (root, handle) = start_registry().await;
    let client = Client::new();

    let plugin: serde_json::Value = client
        .get(format!("{root}/v1/plugins/kelvin.cli"))
        .send()
        .await
        .expect("plugin request")
        .json()
        .await
        .expect("plugin payload");
    assert_eq!(plugin["latest"]["version"], json!("0.10.0"));
    assert_eq!(plugin["versions"][1]["version"], json!("0.1.0"));

    let trust: serde_json::Value = client
        .get(format!("{root}/v1/trust-policy"))
        .send()
        .await
        .expect("trust request")
        .json()
        .await
        .expect("trust payload");
    assert_eq!(trust["trust_policy"]["require_signature"], json!(true));

    handle.abort();
}
