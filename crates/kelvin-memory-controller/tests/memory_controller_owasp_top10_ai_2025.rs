mod common;

use tonic::{Code, Request};

use kelvin_memory_api::v1alpha1::memory_service_server::MemoryService;
use kelvin_memory_api::v1alpha1::{
    HealthRequest, QueryRequest, ReadRequest, RequestContext, UpsertRequest,
};
use kelvin_memory_api::MemoryOperation;

use common::{
    claims_for, context_for, controller_with_module, next_id, sample_manifest, sample_wasm,
    TEST_PRIVATE_KEY_PEM,
};

#[tokio::test]
async fn llm01_prompt_injection_rejects_context_tampering() {
    let controller = controller_with_module(sample_wasm()).await;
    let claims = claims_for(MemoryOperation::Read, &next_id("jti"));
    let mut context = context_for(&claims, &next_id("req"));
    context.workspace_id = "workspace-a\n[[tool:inject]]".to_string();

    let result = controller
        .read(Request::new(ReadRequest {
            context: Some(context),
            key: "MEMORY.md".to_string(),
        }))
        .await
        .expect_err("mismatch should be rejected");
    assert_eq!(result.code(), Code::InvalidArgument);
}

#[tokio::test]
async fn llm02_sensitive_information_disclosure_does_not_echo_token_payload() {
    let controller = controller_with_module(sample_wasm()).await;
    let secret = "TOP_SECRET_MEMORY_TOKEN_123";
    let context = RequestContext {
        delegation_token: format!("header.{secret}.sig"),
        request_id: next_id("req"),
        tenant_id: "tenant-a".to_string(),
        workspace_id: "workspace-a".to_string(),
        session_id: "session-a".to_string(),
        module_id: "memory.echo".to_string(),
    };
    let err = controller
        .health(Request::new(HealthRequest {
            context: Some(context),
        }))
        .await
        .expect_err("invalid token");
    assert_eq!(err.code(), Code::InvalidArgument);
    assert!(
        !err.message().contains(secret),
        "error message should not leak raw token content"
    );
}

#[tokio::test]
async fn llm03_supply_chain_rejects_wrong_audience() {
    let controller = controller_with_module(sample_wasm()).await;
    let mut claims = claims_for(MemoryOperation::Health, &next_id("jti"));
    claims.aud = "wrong-audience".to_string();

    let err = controller
        .health(Request::new(HealthRequest {
            context: Some(context_for(&claims, &next_id("req"))),
        }))
        .await
        .expect_err("audience mismatch should fail");
    assert_eq!(err.code(), Code::InvalidArgument);
}

#[tokio::test]
async fn llm04_data_and_model_poisoning_idempotency_prevents_mutation_on_retry() {
    let controller = controller_with_module(sample_wasm()).await;

    let request_id = next_id("req-idempotent");
    let first = claims_for(MemoryOperation::Upsert, &next_id("jti"));
    controller
        .upsert(Request::new(UpsertRequest {
            context: Some(context_for(&first, &request_id)),
            key: "MEMORY.md".to_string(),
            value: b"first-value".to_vec(),
            metadata: Default::default(),
        }))
        .await
        .expect("first upsert");

    let second = claims_for(MemoryOperation::Upsert, &next_id("jti"));
    controller
        .upsert(Request::new(UpsertRequest {
            context: Some(context_for(&second, &request_id)),
            key: "MEMORY.md".to_string(),
            value: b"poisoned-value".to_vec(),
            metadata: Default::default(),
        }))
        .await
        .expect("idempotent second upsert should return cached response");

    let read_claims = claims_for(MemoryOperation::Read, &next_id("jti"));
    let read = controller
        .read(Request::new(ReadRequest {
            context: Some(context_for(&read_claims, &next_id("req"))),
            key: "MEMORY.md".to_string(),
        }))
        .await
        .expect("read")
        .into_inner();
    assert_eq!(read.value, b"first-value".to_vec());
}

#[tokio::test]
async fn llm05_improper_output_handling_enforces_payload_bounds() {
    let controller = controller_with_module(sample_wasm()).await;
    let mut claims = claims_for(MemoryOperation::Upsert, &next_id("jti"));
    claims.request_limits.max_bytes = 32;

    let err = controller
        .upsert(Request::new(UpsertRequest {
            context: Some(context_for(&claims, &next_id("req"))),
            key: "MEMORY.md".to_string(),
            value: vec![0_u8; 64],
            metadata: Default::default(),
        }))
        .await
        .expect_err("oversized payload must fail");
    assert_eq!(err.code(), Code::InvalidArgument);
}

#[tokio::test]
async fn llm06_excessive_agency_requires_explicit_capability() {
    let controller = controller_with_module(sample_wasm()).await;
    let mut claims = claims_for(MemoryOperation::Read, &next_id("jti"));
    claims.allowed_capabilities = vec!["memory_crud".to_string()];

    let err = controller
        .read(Request::new(ReadRequest {
            context: Some(context_for(&claims, &next_id("req"))),
            key: "MEMORY.md".to_string(),
        }))
        .await
        .expect_err("missing capability should fail");
    assert_eq!(err.code(), Code::InvalidArgument);
    assert!(err.message().contains("missing capability"));
}

#[tokio::test]
async fn llm07_system_prompt_leakage_rejects_oversized_request_id() {
    let controller = controller_with_module(sample_wasm()).await;
    let claims = claims_for(MemoryOperation::Health, &next_id("jti"));
    let long_request_id = "x".repeat(512);
    let err = controller
        .health(Request::new(HealthRequest {
            context: Some(context_for(&claims, &long_request_id)),
        }))
        .await
        .expect_err("oversized request_id should fail");
    assert_eq!(err.code(), Code::InvalidArgument);
    assert!(err.message().contains("request_id exceeds"));
}

#[tokio::test]
async fn llm08_vector_and_embedding_weaknesses_reject_unavailable_provider_feature() {
    let controller = controller_with_module(sample_wasm()).await;
    let err = controller
        .register_module_bytes(
            sample_manifest(vec!["provider_vector_nvidia".to_string()]),
            &sample_wasm(),
        )
        .await
        .expect_err("missing provider should fail");
    assert!(err.to_string().contains("requires unavailable host feature"));
}

#[tokio::test]
async fn llm09_misinformation_controls_keep_query_order_deterministic() {
    let controller = controller_with_module(sample_wasm()).await;

    for (path, body) in [("b.md", "router"), ("a.md", "router")] {
        let claims = claims_for(MemoryOperation::Upsert, &next_id("jti"));
        controller
            .upsert(Request::new(UpsertRequest {
                context: Some(context_for(&claims, &next_id("req"))),
                key: path.to_string(),
                value: body.as_bytes().to_vec(),
                metadata: Default::default(),
            }))
            .await
            .expect("upsert");
    }

    let claims = claims_for(MemoryOperation::Query, &next_id("jti"));
    let response = controller
        .query(Request::new(QueryRequest {
            context: Some(context_for(&claims, &next_id("req"))),
            query: "router".to_string(),
            max_results: 5,
        }))
        .await
        .expect("query")
        .into_inner();
    let paths = response
        .hits
        .into_iter()
        .map(|hit| hit.path)
        .collect::<Vec<_>>();
    assert_eq!(paths, vec!["a.md".to_string(), "b.md".to_string()]);
}

#[tokio::test]
async fn llm10_unbounded_consumption_rejects_excessive_result_window() {
    let controller = controller_with_module(sample_wasm()).await;
    let mut claims = claims_for(MemoryOperation::Query, &next_id("jti"));
    claims.request_limits.max_results = 2;
    let err = controller
        .query(Request::new(QueryRequest {
            context: Some(context_for(&claims, &next_id("req"))),
            query: "router".to_string(),
            max_results: 99,
        }))
        .await
        .expect_err("excessive max_results should fail");
    assert_eq!(err.code(), Code::InvalidArgument);
    assert!(err.message().contains("exceeds limit"));
}

#[test]
fn llm03_supply_chain_rejects_malformed_signing_key_material() {
    let invalid = jsonwebtoken::EncodingKey::from_ed_pem(b"not-a-key");
    assert!(invalid.is_err());
    let valid = jsonwebtoken::EncodingKey::from_ed_pem(TEST_PRIVATE_KEY_PEM.as_bytes());
    assert!(valid.is_ok());
}
