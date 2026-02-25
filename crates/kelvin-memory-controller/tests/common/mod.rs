use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use jsonwebtoken::{EncodingKey, Header};

use kelvin_memory_api::v1alpha1::RequestContext;
use kelvin_memory_api::{
    DelegationClaims, JWT_ALGORITHM, MemoryModuleManifest, MemoryOperation, RequestLimits,
};
use kelvin_memory_controller::{MemoryController, MemoryControllerConfig, ProviderRegistry};

pub const TEST_PRIVATE_KEY_PEM: &str = r#"-----BEGIN PRIVATE KEY-----
MC4CAQAwBQYDK2VwBCIEIHCRmiDXsIoP30rbpS6V729OHS4HzRnpgTwSC9zqETba
-----END PRIVATE KEY-----
"#;

pub const TEST_PUBLIC_KEY_PEM: &str = r#"-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEAHOzip8DiPZOcMhc+e66Wzd1ifXEFAP8DEGUzJFg/DBc=
-----END PUBLIC KEY-----
"#;

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn next_id(prefix: &str) -> String {
    let value = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{prefix}-{value}")
}

pub fn sample_manifest(required_host_features: Vec<String>) -> MemoryModuleManifest {
    MemoryModuleManifest {
        module_id: "memory.echo".to_string(),
        version: "0.1.0".to_string(),
        api_version: "0.1.0".to_string(),
        capabilities: vec![
            "memory_crud".to_string(),
            "memory_read".to_string(),
            "memory_health".to_string(),
        ],
        required_host_features,
        entrypoint: "memory_echo.wasm".to_string(),
        publisher: "acme".to_string(),
        signature: "test-signature".to_string(),
    }
}

pub fn sample_wasm() -> Vec<u8> {
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
          (func (export "handle_upsert") (result i32)
            i32.const 1
            call $kv_put
            drop
            i32.const 0
          )
          (func (export "handle_query") (result i32)
            i32.const 1
            call $kv_get
            drop
            i32.const 0
          )
          (func (export "handle_read") (result i32)
            call $clock
            drop
            i32.const 0
          )
          (func (export "handle_delete") (result i32) i32.const 0)
          (func (export "handle_health") (result i32) i32.const 0)
        )
        "#,
    )
    .expect("compile wat")
}

#[allow(dead_code)]
pub fn busy_loop_wasm() -> Vec<u8> {
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
          (func (export "handle_upsert") (result i32)
            (loop $spin br $spin)
            i32.const 0
          )
          (func (export "handle_query") (result i32) i32.const 0)
          (func (export "handle_read") (result i32) i32.const 0)
          (func (export "handle_delete") (result i32) i32.const 0)
          (func (export "handle_health") (result i32) i32.const 0)
        )
        "#,
    )
    .expect("compile wat")
}

pub async fn controller_with_module(wasm: Vec<u8>) -> Arc<MemoryController> {
    let mut cfg = MemoryControllerConfig::default();
    cfg.decoding_key_pem = TEST_PUBLIC_KEY_PEM.to_string();
    cfg.default_timeout_ms = 150;
    cfg.default_fuel = 5_000;
    let controller = Arc::new(
        MemoryController::new(cfg, ProviderRegistry::with_default_in_memory())
            .expect("controller"),
    );
    controller
        .register_module_bytes(sample_manifest(vec!["provider_sqlite".to_string()]), &wasm)
        .await
        .expect("register module");
    controller
}

pub fn claims_for(operation: MemoryOperation, jti: &str) -> DelegationClaims {
    DelegationClaims {
        iss: "kelvin-root".to_string(),
        sub: "run-1".to_string(),
        aud: "kelvin-memory-controller".to_string(),
        jti: jti.to_string(),
        exp: 4_102_444_800,
        nbf: 1_700_000_000,
        tenant_id: "tenant-a".to_string(),
        workspace_id: "workspace-a".to_string(),
        session_id: "session-a".to_string(),
        module_id: "memory.echo".to_string(),
        allowed_ops: vec![operation.as_str().to_string()],
        allowed_capabilities: vec![
            "memory_crud".to_string(),
            "memory_read".to_string(),
            "memory_health".to_string(),
        ],
        request_limits: RequestLimits {
            timeout_ms: 300,
            max_bytes: 1024,
            max_results: 5,
        },
    }
}

pub fn context_for(claims: &DelegationClaims, request_id: &str) -> RequestContext {
    let key = EncodingKey::from_ed_pem(TEST_PRIVATE_KEY_PEM.as_bytes()).expect("encoding");
    let token =
        jsonwebtoken::encode(&Header::new(JWT_ALGORITHM), claims, &key).expect("encode token");
    RequestContext {
        delegation_token: token,
        request_id: request_id.to_string(),
        tenant_id: claims.tenant_id.clone(),
        workspace_id: claims.workspace_id.clone(),
        session_id: claims.session_id.clone(),
        module_id: claims.module_id.clone(),
    }
}
