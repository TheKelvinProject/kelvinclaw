use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use jsonwebtoken::EncodingKey;
use tokio::sync::Mutex;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};
use tonic::Request;
use url::Url;

use kelvin_core::{
    KelvinError, KelvinResult, MemoryEmbeddingProbeResult, MemoryProviderStatus, MemoryReadParams,
    MemoryReadResult, MemorySearchManager, MemorySearchOptions, MemorySearchResult, MemorySource,
    MemorySyncParams,
};
use kelvin_memory_api::v1alpha1::memory_service_client::MemoryServiceClient;
use kelvin_memory_api::v1alpha1::{
    HealthRequest, QueryRequest, ReadRequest, RequestContext, SearchHit, UpsertRequest,
};
use kelvin_memory_api::{
    mint_delegation_token, new_request_id, DelegationClaims, MemoryOperation, RequestLimits,
};

#[derive(Debug, Clone)]
pub struct MemoryClientConfig {
    pub endpoint: String,
    pub issuer: String,
    pub audience: String,
    pub subject: String,
    pub tenant_id: String,
    pub workspace_id: String,
    pub session_id: String,
    pub module_id: String,
    pub signing_key_pem: String,
    pub signing_key_path: String,
    pub tls_ca_pem: String,
    pub tls_ca_path: String,
    pub tls_domain_name: String,
    pub tls_client_cert_pem: String,
    pub tls_client_cert_path: String,
    pub tls_client_key_pem: String,
    pub tls_client_key_path: String,
    pub allow_insecure_non_loopback: bool,
    pub timeout_ms: u64,
    pub max_bytes: u64,
    pub max_results: u32,
}

impl Default for MemoryClientConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:50051".to_string(),
            issuer: "kelvin-root".to_string(),
            audience: "kelvin-memory-controller".to_string(),
            subject: "kelvin-root-memory-client".to_string(),
            tenant_id: "default".to_string(),
            workspace_id: "default".to_string(),
            session_id: "default".to_string(),
            module_id: "memory.echo".to_string(),
            signing_key_pem: String::new(),
            signing_key_path: String::new(),
            tls_ca_pem: String::new(),
            tls_ca_path: String::new(),
            tls_domain_name: String::new(),
            tls_client_cert_pem: String::new(),
            tls_client_cert_path: String::new(),
            tls_client_key_pem: String::new(),
            tls_client_key_path: String::new(),
            allow_insecure_non_loopback: false,
            timeout_ms: 2_000,
            max_bytes: 1024 * 1024,
            max_results: 20,
        }
    }
}

impl MemoryClientConfig {
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Ok(value) = std::env::var("KELVIN_MEMORY_RPC_ENDPOINT") {
            if !value.trim().is_empty() {
                cfg.endpoint = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_RPC_ISSUER") {
            if !value.trim().is_empty() {
                cfg.issuer = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_RPC_AUDIENCE") {
            if !value.trim().is_empty() {
                cfg.audience = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_RPC_SUBJECT") {
            if !value.trim().is_empty() {
                cfg.subject = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_TENANT_ID") {
            if !value.trim().is_empty() {
                cfg.tenant_id = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_WORKSPACE_ID") {
            if !value.trim().is_empty() {
                cfg.workspace_id = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_SESSION_ID") {
            if !value.trim().is_empty() {
                cfg.session_id = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_MODULE_ID") {
            if !value.trim().is_empty() {
                cfg.module_id = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_SIGNING_KEY_PEM") {
            if !value.trim().is_empty() {
                cfg.signing_key_pem = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_SIGNING_KEY_PATH") {
            if !value.trim().is_empty() {
                cfg.signing_key_path = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_RPC_TLS_CA_PEM") {
            if !value.trim().is_empty() {
                cfg.tls_ca_pem = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_RPC_TLS_CA_PATH") {
            if !value.trim().is_empty() {
                cfg.tls_ca_path = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_RPC_TLS_DOMAIN_NAME") {
            if !value.trim().is_empty() {
                cfg.tls_domain_name = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_RPC_TLS_CLIENT_CERT_PEM") {
            if !value.trim().is_empty() {
                cfg.tls_client_cert_pem = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_RPC_TLS_CLIENT_CERT_PATH") {
            if !value.trim().is_empty() {
                cfg.tls_client_cert_path = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_RPC_TLS_CLIENT_KEY_PEM") {
            if !value.trim().is_empty() {
                cfg.tls_client_key_pem = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_RPC_TLS_CLIENT_KEY_PATH") {
            if !value.trim().is_empty() {
                cfg.tls_client_key_path = value;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_RPC_ALLOW_INSECURE_NON_LOOPBACK") {
            cfg.allow_insecure_non_loopback = parse_bool(value.trim());
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_TIMEOUT_MS") {
            if let Ok(parsed) = value.parse::<u64>() {
                cfg.timeout_ms = parsed;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_MAX_BYTES") {
            if let Ok(parsed) = value.parse::<u64>() {
                cfg.max_bytes = parsed;
            }
        }
        if let Ok(value) = std::env::var("KELVIN_MEMORY_MAX_RESULTS") {
            if let Ok(parsed) = value.parse::<u32>() {
                cfg.max_results = parsed;
            }
        }
        cfg
    }

    pub fn validate(&self) -> KelvinResult<()> {
        validate_required_field("memory rpc endpoint", &self.endpoint)?;
        validate_required_field("memory rpc issuer", &self.issuer)?;
        validate_required_field("memory rpc audience", &self.audience)?;
        validate_required_field("memory rpc subject", &self.subject)?;
        validate_required_field("memory tenant id", &self.tenant_id)?;
        validate_required_field("memory workspace id", &self.workspace_id)?;
        validate_required_field("memory session id", &self.session_id)?;
        validate_required_field("memory module id", &self.module_id)?;
        if self.timeout_ms == 0 {
            return Err(KelvinError::InvalidInput(
                "memory timeout must be > 0".to_string(),
            ));
        }
        if self.max_bytes == 0 {
            return Err(KelvinError::InvalidInput(
                "memory max bytes must be > 0".to_string(),
            ));
        }
        if self.max_results == 0 {
            return Err(KelvinError::InvalidInput(
                "memory max results must be > 0".to_string(),
            ));
        }

        let parsed = Url::parse(self.endpoint.trim()).map_err(|err| {
            KelvinError::InvalidInput(format!(
                "invalid memory rpc endpoint '{}': {err}",
                self.endpoint
            ))
        })?;
        let scheme = parsed.scheme().to_ascii_lowercase();
        if scheme != "http" && scheme != "https" {
            return Err(KelvinError::InvalidInput(
                "memory rpc endpoint must use http:// or https://".to_string(),
            ));
        }
        let host = parsed.host_str().ok_or_else(|| {
            KelvinError::InvalidInput("memory rpc endpoint host is missing".to_string())
        })?;
        if scheme == "http" && !self.allow_insecure_non_loopback && !is_loopback_host(host) {
            return Err(KelvinError::InvalidInput(format!(
                "refusing insecure non-loopback memory rpc endpoint '{host}'. use https:// or set KELVIN_MEMORY_RPC_ALLOW_INSECURE_NON_LOOPBACK=true only on trusted networks"
            )));
        }
        Ok(())
    }
}

pub struct RpcMemoryManager {
    cfg: MemoryClientConfig,
    signer: EncodingKey,
    client: Mutex<MemoryServiceClient<Channel>>,
}

impl RpcMemoryManager {
    pub async fn connect(cfg: MemoryClientConfig) -> KelvinResult<Self> {
        cfg.validate()?;
        let signing_key_pem = resolve_required_pem(
            &cfg.signing_key_pem,
            &cfg.signing_key_path,
            "memory rpc signing key",
        )?;
        let signer = EncodingKey::from_ed_pem(signing_key_pem.as_bytes()).map_err(|err| {
            KelvinError::InvalidInput(format!("invalid memory rpc signing key pem: {err}"))
        })?;
        let endpoint = build_endpoint(&cfg)?;
        let channel = endpoint.connect().await.map_err(|err| {
            KelvinError::Backend(format!(
                "memory controller unavailable at {}: {err}",
                cfg.endpoint
            ))
        })?;
        let client = MemoryServiceClient::new(channel);
        Ok(Self {
            cfg,
            signer,
            client: Mutex::new(client),
        })
    }

    fn build_context(
        &self,
        op: MemoryOperation,
        request_id: String,
    ) -> KelvinResult<RequestContext> {
        let now = now_secs();
        let allowed_capabilities = match op {
            MemoryOperation::Upsert | MemoryOperation::Delete => vec!["memory_crud".to_string()],
            MemoryOperation::Query | MemoryOperation::Read => vec!["memory_read".to_string()],
            MemoryOperation::Health => vec!["memory_health".to_string()],
        };
        let claims = DelegationClaims {
            iss: self.cfg.issuer.clone(),
            sub: self.cfg.subject.clone(),
            aud: self.cfg.audience.clone(),
            jti: format!("{}-{request_id}", op.as_str()),
            exp: now.saturating_add(60),
            nbf: now.saturating_sub(1),
            tenant_id: self.cfg.tenant_id.clone(),
            workspace_id: self.cfg.workspace_id.clone(),
            session_id: self.cfg.session_id.clone(),
            module_id: self.cfg.module_id.clone(),
            allowed_ops: vec![op.as_str().to_string()],
            allowed_capabilities,
            request_limits: RequestLimits {
                timeout_ms: self.cfg.timeout_ms,
                max_bytes: self.cfg.max_bytes,
                max_results: self.cfg.max_results,
            },
        };
        let token = mint_delegation_token(&claims, &self.signer)
            .map_err(|err| KelvinError::InvalidInput(format!("failed to mint token: {err}")))?;
        Ok(RequestContext {
            delegation_token: token,
            request_id,
            tenant_id: self.cfg.tenant_id.clone(),
            workspace_id: self.cfg.workspace_id.clone(),
            session_id: self.cfg.session_id.clone(),
            module_id: self.cfg.module_id.clone(),
        })
    }

    pub async fn upsert(&self, key: &str, value: &[u8]) -> KelvinResult<()> {
        let request_id = new_request_id();
        let context = self.build_context(MemoryOperation::Upsert, request_id)?;
        self.client
            .lock()
            .await
            .upsert(Request::new(UpsertRequest {
                context: Some(context),
                key: key.to_string(),
                value: value.to_vec(),
                metadata: Default::default(),
            }))
            .await
            .map_err(map_status)?;
        Ok(())
    }
}

#[async_trait]
impl MemorySearchManager for RpcMemoryManager {
    async fn search(
        &self,
        query: &str,
        opts: MemorySearchOptions,
    ) -> KelvinResult<Vec<MemorySearchResult>> {
        let request_id = new_request_id();
        let context = self.build_context(MemoryOperation::Query, request_id)?;
        let max_results = u32::try_from(opts.max_results)
            .unwrap_or(self.cfg.max_results)
            .min(self.cfg.max_results);
        let response = self
            .client
            .lock()
            .await
            .query(Request::new(QueryRequest {
                context: Some(context),
                query: query.to_string(),
                max_results,
            }))
            .await
            .map_err(map_status)?
            .into_inner();
        Ok(response
            .hits
            .into_iter()
            .map(map_search_hit)
            .collect::<Vec<_>>())
    }

    async fn read_file(&self, params: MemoryReadParams) -> KelvinResult<MemoryReadResult> {
        let request_id = new_request_id();
        let context = self.build_context(MemoryOperation::Read, request_id)?;
        let response = self
            .client
            .lock()
            .await
            .read(Request::new(ReadRequest {
                context: Some(context),
                key: params.rel_path.clone(),
            }))
            .await
            .map_err(map_status)?
            .into_inner();

        let text = if response.found {
            String::from_utf8(response.value).map_err(|err| {
                KelvinError::Backend(format!(
                    "memory controller returned non-utf8 payload: {err}"
                ))
            })?
        } else {
            String::new()
        };
        Ok(MemoryReadResult {
            text,
            path: params.rel_path,
        })
    }

    fn status(&self) -> MemoryProviderStatus {
        MemoryProviderStatus {
            backend: "rpc".to_string(),
            provider: "kelvin-memory-controller".to_string(),
            model: None,
            requested_provider: Some("memory-controller".to_string()),
            files: None,
            chunks: None,
            dirty: false,
            fallback: None,
            custom: serde_json::json!({
                "endpoint": self.cfg.endpoint,
                "module_id": self.cfg.module_id,
            }),
        }
    }

    async fn sync(&self, _params: Option<MemorySyncParams>) -> KelvinResult<()> {
        let request_id = new_request_id();
        let context = self.build_context(MemoryOperation::Health, request_id)?;
        self.client
            .lock()
            .await
            .health(Request::new(HealthRequest {
                context: Some(context),
            }))
            .await
            .map_err(map_status)?;
        Ok(())
    }

    async fn probe_embedding_availability(&self) -> KelvinResult<MemoryEmbeddingProbeResult> {
        Ok(MemoryEmbeddingProbeResult {
            ok: false,
            error: Some("embedding is provider-specific and not enabled in rpc mvp".to_string()),
        })
    }

    async fn probe_vector_availability(&self) -> KelvinResult<bool> {
        Ok(false)
    }
}

fn parse_bool(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn validate_required_field(label: &str, value: &str) -> KelvinResult<()> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(KelvinError::InvalidInput(format!(
            "{label} must not be empty"
        )));
    }
    if trimmed.chars().any(|ch| ch.is_control()) {
        return Err(KelvinError::InvalidInput(format!(
            "{label} must not include control characters"
        )));
    }
    Ok(())
}

fn is_loopback_host(host: &str) -> bool {
    let normalized = host.trim().to_ascii_lowercase();
    normalized == "localhost" || normalized == "127.0.0.1" || normalized == "::1"
}

fn build_endpoint(cfg: &MemoryClientConfig) -> KelvinResult<Endpoint> {
    let mut endpoint = Endpoint::from_shared(cfg.endpoint.clone()).map_err(|err| {
        KelvinError::InvalidInput(format!(
            "invalid memory rpc endpoint '{}': {err}",
            cfg.endpoint
        ))
    })?;
    let uses_tls = endpoint_uses_tls(&cfg.endpoint);
    let tls_material_configured = !cfg.tls_ca_pem.trim().is_empty()
        || !cfg.tls_ca_path.trim().is_empty()
        || !cfg.tls_domain_name.trim().is_empty()
        || !cfg.tls_client_cert_pem.trim().is_empty()
        || !cfg.tls_client_cert_path.trim().is_empty()
        || !cfg.tls_client_key_pem.trim().is_empty()
        || !cfg.tls_client_key_path.trim().is_empty();

    if uses_tls {
        let domain_name = if cfg.tls_domain_name.trim().is_empty() {
            infer_tls_domain_name(&cfg.endpoint).ok_or_else(|| {
                KelvinError::InvalidInput(
                    "unable to infer tls domain name from memory rpc endpoint; set KELVIN_MEMORY_RPC_TLS_DOMAIN_NAME"
                        .to_string(),
                )
            })?
        } else {
            cfg.tls_domain_name.trim().to_string()
        };

        let mut tls = ClientTlsConfig::new().domain_name(domain_name);
        if let Some(ca_pem) =
            resolve_optional_pem(&cfg.tls_ca_pem, &cfg.tls_ca_path, "memory rpc tls ca")?
        {
            tls = tls.ca_certificate(Certificate::from_pem(ca_pem));
        }

        let client_cert = resolve_optional_pem(
            &cfg.tls_client_cert_pem,
            &cfg.tls_client_cert_path,
            "memory rpc tls client cert",
        )?;
        let client_key = resolve_optional_pem(
            &cfg.tls_client_key_pem,
            &cfg.tls_client_key_path,
            "memory rpc tls client key",
        )?;
        match (client_cert, client_key) {
            (Some(cert), Some(key)) => {
                tls = tls.identity(Identity::from_pem(cert, key));
            }
            (None, None) => {}
            _ => {
                return Err(KelvinError::InvalidInput(
                    "mTLS client identity requires both cert and key".to_string(),
                ));
            }
        }

        endpoint = endpoint.tls_config(tls).map_err(|err| {
            KelvinError::InvalidInput(format!("invalid memory rpc tls config: {err}"))
        })?;
    } else if tls_material_configured {
        return Err(KelvinError::InvalidInput(
            "tls settings provided for non-https memory rpc endpoint".to_string(),
        ));
    }

    Ok(endpoint)
}

fn resolve_required_pem(inline: &str, path: &str, label: &str) -> KelvinResult<String> {
    resolve_optional_pem(inline, path, label)?.ok_or_else(|| {
        KelvinError::InvalidInput(format!("{label} must be provided via inline pem or path"))
    })
}

fn resolve_optional_pem(inline: &str, path: &str, label: &str) -> KelvinResult<Option<String>> {
    let inline = inline.trim();
    let path = path.trim();
    if !inline.is_empty() && !path.is_empty() {
        return Err(KelvinError::InvalidInput(format!(
            "{label} cannot set both inline pem and path"
        )));
    }
    if !inline.is_empty() {
        return Ok(Some(inline.to_string()));
    }
    if path.is_empty() {
        return Ok(None);
    }
    let pem = fs::read_to_string(path).map_err(|err| {
        KelvinError::InvalidInput(format!("{label} path '{path}' is not readable: {err}"))
    })?;
    if pem.trim().is_empty() {
        return Err(KelvinError::InvalidInput(format!(
            "{label} path '{path}' is empty"
        )));
    }
    Ok(Some(pem))
}

fn endpoint_uses_tls(endpoint: &str) -> bool {
    endpoint
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("https://")
}

fn infer_tls_domain_name(endpoint: &str) -> Option<String> {
    let rest = endpoint
        .trim_start()
        .to_ascii_lowercase()
        .strip_prefix("https://")?
        .to_string();
    let authority = rest.split('/').next().unwrap_or_default();
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    if host_port.starts_with('[') {
        let end = host_port.find(']')?;
        let host = &host_port[1..end];
        if host.is_empty() {
            None
        } else {
            Some(host.to_string())
        }
    } else {
        let host = host_port.split(':').next().unwrap_or_default();
        if host.is_empty() {
            None
        } else {
            Some(host.to_string())
        }
    }
}

fn map_search_hit(hit: SearchHit) -> MemorySearchResult {
    MemorySearchResult {
        path: hit.path,
        start_line: hit.start_line as usize,
        end_line: hit.end_line as usize,
        score: hit.score,
        snippet: hit.snippet,
        source: MemorySource::Memory,
        citation: None,
    }
}

fn map_status(status: tonic::Status) -> KelvinError {
    if status.code() == tonic::Code::DeadlineExceeded {
        KelvinError::Timeout(status.message().to_string())
    } else if status.code() == tonic::Code::InvalidArgument {
        KelvinError::InvalidInput(status.message().to_string())
    } else if status.code() == tonic::Code::NotFound {
        KelvinError::NotFound(status.message().to_string())
    } else {
        KelvinError::Backend(format!(
            "memory controller unavailable: {}",
            status.message()
        ))
    }
}

fn now_secs() -> usize {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as usize)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::MemoryClientConfig;

    #[test]
    fn config_validate_rejects_http_non_loopback_by_default() {
        let mut cfg = MemoryClientConfig::default();
        cfg.endpoint = "http://10.0.0.8:50051".to_string();
        cfg.signing_key_pem = "placeholder".to_string();
        let err = cfg
            .validate()
            .expect_err("http non-loopback should fail closed");
        assert!(err.to_string().contains("refusing insecure non-loopback"));
    }

    #[test]
    fn config_validate_accepts_http_non_loopback_with_explicit_opt_in() {
        let mut cfg = MemoryClientConfig::default();
        cfg.endpoint = "http://10.0.0.8:50051".to_string();
        cfg.allow_insecure_non_loopback = true;
        cfg.signing_key_pem = "placeholder".to_string();
        cfg.validate()
            .expect("explicitly allowed insecure non-loopback should pass");
    }

    #[test]
    fn config_validate_rejects_empty_subject() {
        let mut cfg = MemoryClientConfig::default();
        cfg.subject = "   ".to_string();
        cfg.signing_key_pem = "placeholder".to_string();
        let err = cfg.validate().expect_err("empty subject should fail");
        assert!(err.to_string().contains("memory rpc subject"));
    }
}
