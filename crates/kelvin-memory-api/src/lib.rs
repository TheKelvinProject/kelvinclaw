use std::collections::HashSet;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const MEMORY_API_VERSION: &str = "v1alpha1";
pub const JWT_ALGORITHM: Algorithm = Algorithm::EdDSA;

pub mod v1alpha1 {
    tonic::include_proto!("kelvin.memory.v1alpha1");
}

pub const MEMORY_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/kelvin_memory_descriptor.bin"));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryOperation {
    Upsert,
    Query,
    Read,
    Delete,
    Health,
}

impl MemoryOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Upsert => "upsert",
            Self::Query => "query",
            Self::Read => "read",
            Self::Delete => "delete",
            Self::Health => "health",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestLimits {
    pub timeout_ms: u64,
    pub max_bytes: u64,
    pub max_results: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegationClaims {
    pub iss: String,
    pub sub: String,
    pub aud: String,
    pub jti: String,
    pub exp: usize,
    pub nbf: usize,
    pub tenant_id: String,
    pub workspace_id: String,
    pub session_id: String,
    pub module_id: String,
    pub allowed_ops: Vec<String>,
    pub allowed_capabilities: Vec<String>,
    pub request_limits: RequestLimits,
}

impl DelegationClaims {
    pub fn allows_operation(&self, op: MemoryOperation) -> bool {
        self.allowed_ops
            .iter()
            .any(|allowed| allowed == op.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryModuleManifest {
    pub module_id: String,
    pub version: String,
    pub api_version: String,
    pub capabilities: Vec<String>,
    pub required_host_features: Vec<String>,
    pub entrypoint: String,
    pub publisher: String,
    pub signature: String,
}

impl MemoryModuleManifest {
    pub fn validate(&self) -> ApiResult<()> {
        if self.module_id.trim().is_empty() {
            return Err(ApiError::InvalidInput(
                "module_id must not be empty".to_string(),
            ));
        }
        if self.version.trim().is_empty() {
            return Err(ApiError::InvalidInput(
                "version must not be empty".to_string(),
            ));
        }
        if self.api_version.trim().is_empty() {
            return Err(ApiError::InvalidInput(
                "api_version must not be empty".to_string(),
            ));
        }
        if self.entrypoint.trim().is_empty() {
            return Err(ApiError::InvalidInput(
                "entrypoint must not be empty".to_string(),
            ));
        }
        if self.publisher.trim().is_empty() {
            return Err(ApiError::InvalidInput(
                "publisher must not be empty".to_string(),
            ));
        }
        if self.signature.trim().is_empty() {
            return Err(ApiError::InvalidInput(
                "signature must not be empty".to_string(),
            ));
        }

        let mut seen = HashSet::new();
        for capability in &self.capabilities {
            let normalized = capability.trim();
            if normalized.is_empty() {
                return Err(ApiError::InvalidInput(
                    "capabilities must not include empty value".to_string(),
                ));
            }
            if !seen.insert(normalized.to_string()) {
                return Err(ApiError::InvalidInput(format!(
                    "duplicate capability: {normalized}"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("token error: {0}")]
    Token(String),
    #[error("serialization error: {0}")]
    Serialization(String),
}

pub type ApiResult<T> = Result<T, ApiError>;

pub fn new_request_id() -> String {
    Uuid::new_v4().to_string()
}

pub fn delegation_token_signing_input(claims: &DelegationClaims) -> ApiResult<String> {
    let header = Header::new(JWT_ALGORITHM);
    let header_json =
        serde_json::to_vec(&header).map_err(|err| ApiError::Serialization(err.to_string()))?;
    let claims_json =
        serde_json::to_vec(claims).map_err(|err| ApiError::Serialization(err.to_string()))?;
    Ok(format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header_json),
        URL_SAFE_NO_PAD.encode(claims_json)
    ))
}

pub fn format_signed_delegation_token(signing_input: &str, signature: &[u8]) -> ApiResult<String> {
    if signing_input.trim().is_empty() {
        return Err(ApiError::InvalidInput(
            "delegation signing input must not be empty".to_string(),
        ));
    }
    if signature.is_empty() {
        return Err(ApiError::InvalidInput(
            "delegation signature must not be empty".to_string(),
        ));
    }
    Ok(format!(
        "{}.{}",
        signing_input,
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

pub fn mint_delegation_token(claims: &DelegationClaims, key: &EncodingKey) -> ApiResult<String> {
    let header = Header::new(JWT_ALGORITHM);
    encode(&header, claims, key).map_err(|err| ApiError::Token(err.to_string()))
}

pub fn verify_delegation_token(
    token: &str,
    key: &DecodingKey,
    expected_issuer: &str,
    expected_audience: &str,
    clock_skew_secs: u64,
) -> ApiResult<DelegationClaims> {
    let mut validation = Validation::new(JWT_ALGORITHM);
    validation.leeway = clock_skew_secs;
    validation.validate_nbf = true;
    validation.set_issuer(&[expected_issuer]);
    validation.set_audience(&[expected_audience]);

    let data = decode::<DelegationClaims>(token, key, &validation)
        .map_err(|err| ApiError::Token(err.to_string()))?;
    Ok(data.claims)
}

#[cfg(test)]
mod tests {
    use prost::Message;
    use prost_types::FileDescriptorSet;

    use super::{
        delegation_token_signing_input, mint_delegation_token, verify_delegation_token,
        DelegationClaims, MemoryOperation, RequestLimits, MEMORY_DESCRIPTOR_SET,
    };
    use jsonwebtoken::{DecodingKey, EncodingKey};

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

    fn sample_claims() -> DelegationClaims {
        DelegationClaims {
            iss: "kelvin-root".to_string(),
            sub: "run-1".to_string(),
            aud: "kelvin-memory-controller".to_string(),
            jti: "token-1".to_string(),
            exp: 4_102_444_800,
            nbf: 1_700_000_000,
            tenant_id: "tenant-a".to_string(),
            workspace_id: "workspace-a".to_string(),
            session_id: "session-a".to_string(),
            module_id: "memory.echo".to_string(),
            allowed_ops: vec![
                MemoryOperation::Upsert.as_str().to_string(),
                MemoryOperation::Query.as_str().to_string(),
            ],
            allowed_capabilities: vec!["memory_crud".to_string()],
            request_limits: RequestLimits {
                timeout_ms: 1000,
                max_bytes: 1024,
                max_results: 5,
            },
        }
    }

    #[test]
    fn mint_and_verify_roundtrip() {
        let claims = sample_claims();
        let private_key = test_private_key_pem();
        let encoding = EncodingKey::from_ed_pem(private_key.as_bytes()).expect("encoding");
        let token = mint_delegation_token(&claims, &encoding).expect("mint token");
        let public_key = test_public_key_pem();
        let decoding = DecodingKey::from_ed_pem(public_key.as_bytes()).expect("decoding");
        let parsed = verify_delegation_token(
            &token,
            &decoding,
            "kelvin-root",
            "kelvin-memory-controller",
            30,
        )
        .expect("verify");
        assert_eq!(parsed.module_id, "memory.echo");
        assert!(parsed.allows_operation(MemoryOperation::Query));
    }

    #[test]
    fn signing_input_has_expected_jwt_shape() {
        let input = delegation_token_signing_input(&sample_claims()).expect("signing input");
        let segments = input.split('.').collect::<Vec<_>>();
        assert_eq!(segments.len(), 2);
        assert!(segments.iter().all(|segment| !segment.is_empty()));
    }

    #[test]
    fn descriptor_contract_contains_memory_service_surface() {
        let descriptor =
            FileDescriptorSet::decode(MEMORY_DESCRIPTOR_SET).expect("decode descriptor set");
        let file = descriptor
            .file
            .iter()
            .find(|item| item.package.as_deref() == Some("kelvin.memory.v1alpha1"))
            .expect("memory package");
        let service = file
            .service
            .iter()
            .find(|item| item.name.as_deref() == Some("MemoryService"))
            .expect("memory service");

        let mut methods = service
            .method
            .iter()
            .map(|item| item.name.clone().unwrap_or_default())
            .collect::<Vec<_>>();
        methods.sort();
        assert_eq!(methods, vec!["Delete", "Health", "Query", "Read", "Upsert"]);
    }

    #[test]
    fn descriptor_contract_keeps_request_context_field_numbers_stable() {
        let descriptor =
            FileDescriptorSet::decode(MEMORY_DESCRIPTOR_SET).expect("decode descriptor set");
        let file = descriptor
            .file
            .iter()
            .find(|item| item.package.as_deref() == Some("kelvin.memory.v1alpha1"))
            .expect("memory package");
        let request_context = file
            .message_type
            .iter()
            .find(|item| item.name.as_deref() == Some("RequestContext"))
            .expect("request context");
        let mut fields = request_context
            .field
            .iter()
            .map(|item| {
                (
                    item.name.clone().unwrap_or_default(),
                    item.number.unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>();
        fields.sort_by(|a, b| a.1.cmp(&b.1));
        assert_eq!(
            fields,
            vec![
                ("delegation_token".to_string(), 1),
                ("request_id".to_string(), 2),
                ("tenant_id".to_string(), 3),
                ("workspace_id".to_string(), 4),
                ("session_id".to_string(), 5),
                ("module_id".to_string(), 6),
            ]
        );
    }
}
