mod common;

use tonic::{Code, Request};

use kelvin_memory_api::v1alpha1::memory_service_server::MemoryService;
use kelvin_memory_api::v1alpha1::{
    DeleteRequest, HealthRequest, ReadRequest, RequestContext, UpsertRequest,
};
use kelvin_memory_api::MemoryOperation;
use kelvin_memory_controller::{
    MemoryController, MemoryControllerConfig, ProviderProfile, ProviderRegistry,
};

use common::{
    busy_loop_wasm, claims_for, context_for, controller_with_module, next_id, sample_manifest,
    sample_wasm, TEST_PUBLIC_KEY_PEM,
};

// GOVERN: policy and profile governance.
#[test]
fn govern_default_build_profile_remains_minimal_and_non_nvidia() {
    let features = ProviderRegistry::with_default_in_memory().available_features();
    assert!(features.contains(&"provider_sqlite".to_string()));
    assert!(!features.contains(&"provider_vector_nvidia".to_string()));
}

#[cfg(not(feature = "provider_vector_nvidia"))]
#[test]
fn govern_profile_mismatch_is_rejected_when_nvidia_is_unavailable() {
    let mut cfg = MemoryControllerConfig::default();
    cfg.decoding_key_pem = TEST_PUBLIC_KEY_PEM.to_string();
    cfg.profile = ProviderProfile::LinuxGpu;
    let err = match MemoryController::new(cfg, ProviderRegistry::with_default_in_memory()) {
        Ok(_) => panic!("linux-gpu profile without nvidia should fail"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("requires provider_vector_nvidia"));
}

#[tokio::test]
async fn govern_issuer_and_audience_pinning_are_enforced() {
    let controller = controller_with_module(sample_wasm()).await;

    let mut wrong_issuer = claims_for(MemoryOperation::Health, &next_id("jti"));
    wrong_issuer.iss = "wrong-issuer".to_string();
    let err = controller
        .health(Request::new(HealthRequest {
            context: Some(context_for(&wrong_issuer, &next_id("req"))),
        }))
        .await
        .expect_err("issuer mismatch");
    assert_eq!(err.code(), Code::InvalidArgument);

    let mut wrong_audience = claims_for(MemoryOperation::Health, &next_id("jti"));
    wrong_audience.aud = "wrong-audience".to_string();
    let err = controller
        .health(Request::new(HealthRequest {
            context: Some(context_for(&wrong_audience, &next_id("req"))),
        }))
        .await
        .expect_err("audience mismatch");
    assert_eq!(err.code(), Code::InvalidArgument);
}

// MAP: context and scope mapping.
#[tokio::test]
async fn map_claim_and_request_scope_must_match() {
    let controller = controller_with_module(sample_wasm()).await;
    let claims = claims_for(MemoryOperation::Read, &next_id("jti"));
    let mut context = context_for(&claims, &next_id("req"));
    context.tenant_id = "other-tenant".to_string();

    let err = controller
        .read(Request::new(ReadRequest {
            context: Some(context),
            key: "k".to_string(),
        }))
        .await
        .expect_err("scope mismatch");
    assert_eq!(err.code(), Code::InvalidArgument);
}

#[tokio::test]
async fn map_operation_scope_must_be_explicitly_delegated() {
    let controller = controller_with_module(sample_wasm()).await;
    let claims = claims_for(MemoryOperation::Read, &next_id("jti"));
    let err = controller
        .delete(Request::new(DeleteRequest {
            context: Some(context_for(&claims, &next_id("req"))),
            key: "k".to_string(),
        }))
        .await
        .expect_err("operation mismatch");
    assert_eq!(err.code(), Code::InvalidArgument);
    assert!(err.message().contains("does not allow operation"));
}

#[tokio::test]
async fn map_module_manifest_validation_rejects_bad_shapes() {
    let controller = controller_with_module(sample_wasm()).await;
    let mut manifest = sample_manifest(vec!["provider_sqlite".to_string()]);
    manifest.capabilities.push("memory_read".to_string());
    let err = controller
        .register_module_bytes(manifest, &sample_wasm())
        .await
        .expect_err("duplicate capability should fail");
    assert!(err.to_string().contains("duplicate capability"));
}

// MEASURE: reproducible signals and typed observability.
#[tokio::test]
async fn measure_health_reports_enabled_features_and_loaded_modules() {
    let controller = controller_with_module(sample_wasm()).await;
    let claims = claims_for(MemoryOperation::Health, &next_id("jti"));
    let health = controller
        .health(Request::new(HealthRequest {
            context: Some(context_for(&claims, &next_id("req"))),
        }))
        .await
        .expect("health")
        .into_inner();
    assert!(health.ok);
    assert!(health.enabled_features.contains(&"provider_sqlite".to_string()));
    assert!(health.loaded_modules.contains(&"memory.echo".to_string()));
}

#[tokio::test]
async fn measure_not_found_errors_are_typed_for_unknown_modules() {
    let controller = controller_with_module(sample_wasm()).await;
    let mut claims = claims_for(MemoryOperation::Read, &next_id("jti"));
    claims.module_id = "memory.unknown".to_string();
    let context = RequestContext {
        module_id: "memory.unknown".to_string(),
        ..context_for(&claims, &next_id("req"))
    };
    let err = controller
        .read(Request::new(ReadRequest {
            context: Some(context),
            key: "k".to_string(),
        }))
        .await
        .expect_err("unknown module should map to not_found");
    assert_eq!(err.code(), Code::NotFound);
}

#[tokio::test]
async fn measure_idempotent_retries_return_stable_responses() {
    let controller = controller_with_module(sample_wasm()).await;
    let request_id = next_id("req");
    let first = claims_for(MemoryOperation::Upsert, &next_id("jti"));
    let first_resp = controller
        .upsert(Request::new(UpsertRequest {
            context: Some(context_for(&first, &request_id)),
            key: "k".to_string(),
            value: b"v1".to_vec(),
            metadata: Default::default(),
        }))
        .await
        .expect("first")
        .into_inner();

    let second = claims_for(MemoryOperation::Upsert, &next_id("jti"));
    let second_resp = controller
        .upsert(Request::new(UpsertRequest {
            context: Some(context_for(&second, &request_id)),
            key: "k".to_string(),
            value: b"v2".to_vec(),
            metadata: Default::default(),
        }))
        .await
        .expect("second")
        .into_inner();

    assert_eq!(first_resp.stored, second_resp.stored);
}

// MANAGE: resilience and response controls.
#[tokio::test]
async fn manage_replay_protection_blocks_reused_jti() {
    let controller = controller_with_module(sample_wasm()).await;
    let jti = next_id("jti-replay");
    let first = claims_for(MemoryOperation::Upsert, &jti);
    controller
        .upsert(Request::new(UpsertRequest {
            context: Some(context_for(&first, &next_id("req"))),
            key: "k".to_string(),
            value: b"v".to_vec(),
            metadata: Default::default(),
        }))
        .await
        .expect("first request");

    let second = claims_for(MemoryOperation::Upsert, &jti);
    let err = controller
        .upsert(Request::new(UpsertRequest {
            context: Some(context_for(&second, &next_id("req"))),
            key: "k".to_string(),
            value: b"v".to_vec(),
            metadata: Default::default(),
        }))
        .await
        .expect_err("replay should fail");
    assert_eq!(err.code(), Code::InvalidArgument);
}

#[tokio::test]
async fn manage_timeout_controls_contain_busy_modules() {
    let controller = controller_with_module(busy_loop_wasm()).await;
    let claims = claims_for(MemoryOperation::Upsert, &next_id("jti"));
    let err = controller
        .upsert(Request::new(UpsertRequest {
            context: Some(context_for(&claims, &next_id("req"))),
            key: "k".to_string(),
            value: b"v".to_vec(),
            metadata: Default::default(),
        }))
        .await
        .expect_err("busy module should fail");
    assert!(
        err.code() == Code::DeadlineExceeded || err.code() == Code::Unavailable,
        "unexpected code: {} / {}",
        err.code(),
        err.message()
    );
}

#[tokio::test]
async fn manage_request_limits_block_large_upserts() {
    let controller = controller_with_module(sample_wasm()).await;
    let mut claims = claims_for(MemoryOperation::Upsert, &next_id("jti"));
    claims.request_limits.max_bytes = 8;
    let err = controller
        .upsert(Request::new(UpsertRequest {
            context: Some(context_for(&claims, &next_id("req"))),
            key: "k".to_string(),
            value: vec![7_u8; 64],
            metadata: Default::default(),
        }))
        .await
        .expect_err("payload limit should fail");
    assert_eq!(err.code(), Code::InvalidArgument);
}
