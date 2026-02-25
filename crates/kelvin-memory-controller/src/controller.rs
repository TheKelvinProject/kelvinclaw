use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::{Mutex, RwLock};
use tonic::{Request, Response, Status};

use kelvin_core::{KelvinError, KelvinResult};
use kelvin_memory_api::v1alpha1::memory_service_server::MemoryService;
use kelvin_memory_api::v1alpha1::{
    DeleteRequest, DeleteResponse, HealthRequest, HealthResponse, QueryRequest, QueryResponse,
    ReadRequest, ReadResponse, RequestContext, UpsertRequest, UpsertResponse,
};
use kelvin_memory_api::{
    verify_delegation_token, DelegationClaims, MemoryModuleManifest, MemoryOperation,
};
use kelvin_memory_module_sdk::ModuleOperation;

use crate::config::{MemoryControllerConfig, ProviderProfile};
use crate::module_runtime::{LoadedMemoryModule, ModuleRuntimeConfig};
use crate::provider::ProviderRegistry;

#[derive(Default)]
pub struct ReplayCache {
    entries: Mutex<HashMap<String, usize>>,
}

impl ReplayCache {
    pub async fn insert_or_reject(&self, jti: &str, exp: usize) -> KelvinResult<()> {
        let now = now_secs();
        let mut entries = self.entries.lock().await;
        entries.retain(|_, existing_exp| *existing_exp > now);
        if entries.contains_key(jti) {
            return Err(KelvinError::InvalidInput(format!(
                "replayed delegation token jti '{jti}'"
            )));
        }
        entries.insert(jti.to_string(), exp);
        Ok(())
    }
}

#[derive(Clone)]
enum CachedResponse {
    Upsert(UpsertResponse),
    Query(QueryResponse),
    Read(ReadResponse),
    Delete(DeleteResponse),
    Health(HealthResponse),
}

#[derive(Clone)]
struct ValidatedContext {
    request_id: String,
    module_id: String,
    claims: DelegationClaims,
}

pub struct MemoryController {
    config: MemoryControllerConfig,
    decoding_key: jsonwebtoken::DecodingKey,
    providers: ProviderRegistry,
    modules: RwLock<HashMap<String, Arc<LoadedMemoryModule>>>,
    replay_cache: ReplayCache,
    idempotency: Mutex<HashMap<String, CachedResponse>>,
}

impl MemoryController {
    pub fn new(config: MemoryControllerConfig, providers: ProviderRegistry) -> KelvinResult<Self> {
        validate_profile_compatibility(config.profile)?;
        Ok(Self {
            decoding_key: config.decoding_key()?,
            config,
            providers,
            modules: RwLock::new(HashMap::new()),
            replay_cache: ReplayCache::default(),
            idempotency: Mutex::new(HashMap::new()),
        })
    }

    pub async fn register_module_bytes(
        &self,
        manifest: MemoryModuleManifest,
        wasm_bytes: &[u8],
    ) -> KelvinResult<()> {
        enforce_required_host_features(&manifest, &self.providers.available_features())?;
        let runtime_cfg = ModuleRuntimeConfig {
            max_module_bytes: self.config.max_module_bytes,
            max_memory_pages: self.config.max_memory_pages,
            default_fuel: self.config.default_fuel,
            default_timeout_ms: self.config.default_timeout_ms,
        };
        let module = LoadedMemoryModule::new(manifest.clone(), wasm_bytes, runtime_cfg)?;
        self.modules
            .write()
            .await
            .insert(manifest.module_id.clone(), Arc::new(module));
        Ok(())
    }

    pub async fn loaded_modules(&self) -> Vec<String> {
        let mut items = self
            .modules
            .read()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        items.sort();
        items
    }

    async fn validate_context(
        &self,
        context: Option<RequestContext>,
        operation: MemoryOperation,
    ) -> KelvinResult<ValidatedContext> {
        let context = context.ok_or_else(|| {
            KelvinError::InvalidInput("request context is required for memory rpc".to_string())
        })?;
        if context.request_id.trim().is_empty() {
            return Err(KelvinError::InvalidInput(
                "request_id must not be empty".to_string(),
            ));
        }
        if context.request_id.len() > 256 {
            return Err(KelvinError::InvalidInput(
                "request_id exceeds 256 chars".to_string(),
            ));
        }
        let claims = verify_delegation_token(
            &context.delegation_token,
            &self.decoding_key,
            &self.config.issuer,
            &self.config.audience,
            self.config.clock_skew_secs,
        )
        .map_err(|err| KelvinError::InvalidInput(format!("delegation token rejected: {err}")))?;

        if !claims.allows_operation(operation) {
            return Err(KelvinError::InvalidInput(format!(
                "delegation token does not allow operation '{}'",
                operation.as_str()
            )));
        }
        if context.tenant_id != claims.tenant_id
            || context.workspace_id != claims.workspace_id
            || context.session_id != claims.session_id
            || context.module_id != claims.module_id
        {
            return Err(KelvinError::InvalidInput(
                "request context does not match delegation claims".to_string(),
            ));
        }
        let now = now_secs();
        let replay_exp = if self.config.replay_window_secs == 0 {
            claims.exp
        } else {
            claims
                .exp
                .min(now.saturating_add(self.config.replay_window_secs as usize))
        };
        self.replay_cache
            .insert_or_reject(&claims.jti, replay_exp)
            .await?;

        Ok(ValidatedContext {
            request_id: context.request_id,
            module_id: context.module_id,
            claims,
        })
    }

    async fn run_module(
        &self,
        module_id: &str,
        operation: ModuleOperation,
        claims: &DelegationClaims,
        required_capability: &str,
    ) -> KelvinResult<()> {
        if !claims
            .allowed_capabilities
            .iter()
            .any(|cap| cap == required_capability)
        {
            return Err(KelvinError::InvalidInput(format!(
                "delegation token missing capability '{}'",
                required_capability
            )));
        }

        let module = self
            .modules
            .read()
            .await
            .get(module_id)
            .cloned()
            .ok_or_else(|| {
                KelvinError::NotFound(format!("memory module not found: {module_id}"))
            })?;
        if !module
            .manifest()
            .capabilities
            .iter()
            .any(|cap| cap == required_capability)
        {
            return Err(KelvinError::InvalidInput(format!(
                "module '{}' missing required capability '{}'",
                module_id, required_capability
            )));
        }

        module
            .execute(
                operation,
                Some(
                    claims
                        .request_limits
                        .timeout_ms
                        .min(self.config.default_timeout_ms),
                ),
                Some(self.config.default_fuel),
            )
            .await
    }

    async fn check_cached<T>(
        &self,
        request_id: &str,
        map_fn: fn(CachedResponse) -> Option<T>,
    ) -> Option<T> {
        self.idempotency
            .lock()
            .await
            .get(request_id)
            .cloned()
            .and_then(map_fn)
    }

    async fn cache_response(&self, request_id: String, response: CachedResponse) {
        self.idempotency.lock().await.insert(request_id, response);
    }

    fn enforce_response_size(
        &self,
        data_len: usize,
        claims: &DelegationClaims,
    ) -> KelvinResult<()> {
        let claim_limit = usize::try_from(claims.request_limits.max_bytes).unwrap_or(usize::MAX);
        let hard_limit = self.config.default_max_response_bytes.min(claim_limit);
        if data_len > hard_limit {
            return Err(KelvinError::InvalidInput(format!(
                "response payload {} bytes exceeds limit {}",
                data_len, hard_limit
            )));
        }
        Ok(())
    }

    fn audit(
        &self,
        operation: MemoryOperation,
        claims: &DelegationClaims,
        request_id: &str,
        allowed: bool,
        reason: &str,
        started_at_ms: u128,
    ) {
        let latency_ms = now_ms().saturating_sub(started_at_ms);
        let line = serde_json::json!({
            "request_id": request_id,
            "module_id": claims.module_id,
            "tenant_id": claims.tenant_id,
            "workspace_id": claims.workspace_id,
            "session_id": claims.session_id,
            "operation": operation.as_str(),
            "allowed": allowed,
            "reason": reason,
            "latency_ms": latency_ms,
        });
        println!("{line}");
    }
}

#[tonic::async_trait]
impl MemoryService for MemoryController {
    async fn upsert(
        &self,
        request: Request<UpsertRequest>,
    ) -> Result<Response<UpsertResponse>, Status> {
        let started = now_ms();
        let request = request.into_inner();
        let validated = self
            .validate_context(request.context, MemoryOperation::Upsert)
            .await
            .map_err(to_status)?;

        if let Some(cached) = self
            .check_cached(&validated.request_id, |item| match item {
                CachedResponse::Upsert(value) => Some(value),
                _ => None,
            })
            .await
        {
            self.audit(
                MemoryOperation::Upsert,
                &validated.claims,
                &validated.request_id,
                true,
                "idempotency_cache_hit",
                started,
            );
            return Ok(Response::new(cached));
        }

        let max_bytes =
            usize::try_from(validated.claims.request_limits.max_bytes).unwrap_or(usize::MAX);
        if request.value.len() > max_bytes {
            self.audit(
                MemoryOperation::Upsert,
                &validated.claims,
                &validated.request_id,
                false,
                "payload_too_large",
                started,
            );
            return Err(to_status(KelvinError::InvalidInput(format!(
                "upsert payload {} exceeds limit {}",
                request.value.len(),
                max_bytes
            ))));
        }

        let result = async {
            self.run_module(
                &validated.module_id,
                ModuleOperation::Upsert,
                &validated.claims,
                "memory_crud",
            )
            .await?;
            let provider = self.providers.primary()?;
            provider
                .upsert(&request.key, &request.value, &request.metadata)
                .await?;
            Ok::<UpsertResponse, KelvinError>(UpsertResponse { stored: true })
        }
        .await;

        match result {
            Ok(response) => {
                self.cache_response(
                    validated.request_id.clone(),
                    CachedResponse::Upsert(response),
                )
                .await;
                self.audit(
                    MemoryOperation::Upsert,
                    &validated.claims,
                    &validated.request_id,
                    true,
                    "ok",
                    started,
                );
                Ok(Response::new(response))
            }
            Err(err) => {
                self.audit(
                    MemoryOperation::Upsert,
                    &validated.claims,
                    &validated.request_id,
                    false,
                    &err.to_string(),
                    started,
                );
                Err(to_status(err))
            }
        }
    }

    async fn query(
        &self,
        request: Request<QueryRequest>,
    ) -> Result<Response<QueryResponse>, Status> {
        let started = now_ms();
        let request = request.into_inner();
        let validated = self
            .validate_context(request.context, MemoryOperation::Query)
            .await
            .map_err(to_status)?;

        if let Some(cached) = self
            .check_cached(&validated.request_id, |item| match item {
                CachedResponse::Query(value) => Some(value),
                _ => None,
            })
            .await
        {
            self.audit(
                MemoryOperation::Query,
                &validated.claims,
                &validated.request_id,
                true,
                "idempotency_cache_hit",
                started,
            );
            return Ok(Response::new(cached));
        }

        if request.max_results > validated.claims.request_limits.max_results {
            return Err(to_status(KelvinError::InvalidInput(format!(
                "max_results {} exceeds limit {}",
                request.max_results, validated.claims.request_limits.max_results
            ))));
        }

        let result = async {
            self.run_module(
                &validated.module_id,
                ModuleOperation::Query,
                &validated.claims,
                "memory_read",
            )
            .await?;
            let provider = self.providers.primary()?;
            let hits = provider.query(&request.query, request.max_results).await?;
            let total_bytes = hits
                .iter()
                .map(|hit| hit.path.len() + hit.snippet.len())
                .sum::<usize>();
            self.enforce_response_size(total_bytes, &validated.claims)?;
            Ok::<QueryResponse, KelvinError>(QueryResponse { hits })
        }
        .await;

        match result {
            Ok(response) => {
                self.cache_response(
                    validated.request_id.clone(),
                    CachedResponse::Query(response.clone()),
                )
                .await;
                self.audit(
                    MemoryOperation::Query,
                    &validated.claims,
                    &validated.request_id,
                    true,
                    "ok",
                    started,
                );
                Ok(Response::new(response))
            }
            Err(err) => {
                self.audit(
                    MemoryOperation::Query,
                    &validated.claims,
                    &validated.request_id,
                    false,
                    &err.to_string(),
                    started,
                );
                Err(to_status(err))
            }
        }
    }

    async fn read(&self, request: Request<ReadRequest>) -> Result<Response<ReadResponse>, Status> {
        let started = now_ms();
        let request = request.into_inner();
        let validated = self
            .validate_context(request.context, MemoryOperation::Read)
            .await
            .map_err(to_status)?;

        if let Some(cached) = self
            .check_cached(&validated.request_id, |item| match item {
                CachedResponse::Read(value) => Some(value),
                _ => None,
            })
            .await
        {
            self.audit(
                MemoryOperation::Read,
                &validated.claims,
                &validated.request_id,
                true,
                "idempotency_cache_hit",
                started,
            );
            return Ok(Response::new(cached));
        }

        let result = async {
            self.run_module(
                &validated.module_id,
                ModuleOperation::Read,
                &validated.claims,
                "memory_read",
            )
            .await?;
            let provider = self.providers.primary()?;
            let value = provider.read(&request.key).await?;
            let payload_len = value.as_ref().map(|v| v.len()).unwrap_or_default();
            self.enforce_response_size(payload_len, &validated.claims)?;
            Ok::<ReadResponse, KelvinError>(ReadResponse {
                found: value.is_some(),
                value: value.unwrap_or_default(),
            })
        }
        .await;

        match result {
            Ok(response) => {
                self.cache_response(
                    validated.request_id.clone(),
                    CachedResponse::Read(response.clone()),
                )
                .await;
                self.audit(
                    MemoryOperation::Read,
                    &validated.claims,
                    &validated.request_id,
                    true,
                    "ok",
                    started,
                );
                Ok(Response::new(response))
            }
            Err(err) => {
                self.audit(
                    MemoryOperation::Read,
                    &validated.claims,
                    &validated.request_id,
                    false,
                    &err.to_string(),
                    started,
                );
                Err(to_status(err))
            }
        }
    }

    async fn delete(
        &self,
        request: Request<DeleteRequest>,
    ) -> Result<Response<DeleteResponse>, Status> {
        let started = now_ms();
        let request = request.into_inner();
        let validated = self
            .validate_context(request.context, MemoryOperation::Delete)
            .await
            .map_err(to_status)?;

        if let Some(cached) = self
            .check_cached(&validated.request_id, |item| match item {
                CachedResponse::Delete(value) => Some(value),
                _ => None,
            })
            .await
        {
            self.audit(
                MemoryOperation::Delete,
                &validated.claims,
                &validated.request_id,
                true,
                "idempotency_cache_hit",
                started,
            );
            return Ok(Response::new(cached));
        }

        let result = async {
            self.run_module(
                &validated.module_id,
                ModuleOperation::Delete,
                &validated.claims,
                "memory_crud",
            )
            .await?;
            let provider = self.providers.primary()?;
            let deleted = provider.delete(&request.key).await?;
            Ok::<DeleteResponse, KelvinError>(DeleteResponse { deleted })
        }
        .await;

        match result {
            Ok(response) => {
                self.cache_response(
                    validated.request_id.clone(),
                    CachedResponse::Delete(response),
                )
                .await;
                self.audit(
                    MemoryOperation::Delete,
                    &validated.claims,
                    &validated.request_id,
                    true,
                    "ok",
                    started,
                );
                Ok(Response::new(response))
            }
            Err(err) => {
                self.audit(
                    MemoryOperation::Delete,
                    &validated.claims,
                    &validated.request_id,
                    false,
                    &err.to_string(),
                    started,
                );
                Err(to_status(err))
            }
        }
    }

    async fn health(
        &self,
        request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        let started = now_ms();
        let request = request.into_inner();
        let validated = self
            .validate_context(request.context, MemoryOperation::Health)
            .await
            .map_err(to_status)?;

        if let Some(cached) = self
            .check_cached(&validated.request_id, |item| match item {
                CachedResponse::Health(value) => Some(value),
                _ => None,
            })
            .await
        {
            self.audit(
                MemoryOperation::Health,
                &validated.claims,
                &validated.request_id,
                true,
                "idempotency_cache_hit",
                started,
            );
            return Ok(Response::new(cached));
        }

        let result = async {
            self.run_module(
                &validated.module_id,
                ModuleOperation::Health,
                &validated.claims,
                "memory_health",
            )
            .await?;
            let provider = self.providers.primary()?;
            let healthy = provider.health().await?;
            Ok::<HealthResponse, KelvinError>(HealthResponse {
                ok: healthy,
                provider: provider.id().to_string(),
                enabled_features: self.providers.available_features(),
                loaded_modules: self.loaded_modules().await,
            })
        }
        .await;

        match result {
            Ok(response) => {
                self.cache_response(
                    validated.request_id.clone(),
                    CachedResponse::Health(response.clone()),
                )
                .await;
                self.audit(
                    MemoryOperation::Health,
                    &validated.claims,
                    &validated.request_id,
                    true,
                    "ok",
                    started,
                );
                Ok(Response::new(response))
            }
            Err(err) => {
                self.audit(
                    MemoryOperation::Health,
                    &validated.claims,
                    &validated.request_id,
                    false,
                    &err.to_string(),
                    started,
                );
                Err(to_status(err))
            }
        }
    }
}

fn enforce_required_host_features(
    manifest: &MemoryModuleManifest,
    available_features: &[String],
) -> KelvinResult<()> {
    let available = available_features.iter().cloned().collect::<HashSet<_>>();
    for required in &manifest.required_host_features {
        if !available.contains(required) {
            return Err(KelvinError::InvalidInput(format!(
                "module '{}' requires unavailable host feature '{}'",
                manifest.module_id, required
            )));
        }
    }
    Ok(())
}

fn validate_profile_compatibility(profile: ProviderProfile) -> KelvinResult<()> {
    match profile {
        ProviderProfile::Minimal => {
            if cfg!(feature = "provider_vector_nvidia") {
                return Err(KelvinError::InvalidInput(
                    "profile_minimal must not include provider_vector_nvidia".to_string(),
                ));
            }
        }
        ProviderProfile::IPhone => {
            if cfg!(feature = "provider_vector_nvidia") {
                return Err(KelvinError::InvalidInput(
                    "profile_iphone must not include provider_vector_nvidia".to_string(),
                ));
            }
        }
        ProviderProfile::LinuxGpu => {
            if !cfg!(feature = "provider_vector_nvidia") {
                return Err(KelvinError::InvalidInput(
                    "profile_linux_gpu requires provider_vector_nvidia".to_string(),
                ));
            }
        }
    }
    Ok(())
}

fn to_status(err: KelvinError) -> Status {
    match err {
        KelvinError::InvalidInput(message) => Status::invalid_argument(message),
        KelvinError::NotFound(message) => Status::not_found(message),
        KelvinError::Timeout(message) => Status::deadline_exceeded(message),
        KelvinError::Backend(message) => Status::unavailable(message),
        KelvinError::Io(message) => Status::internal(message),
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or_default()
}

fn now_secs() -> usize {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as usize)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use jsonwebtoken::{EncodingKey, Header};
    use tonic::Request;

    use kelvin_memory_api::v1alpha1::memory_service_server::MemoryService;
    use kelvin_memory_api::v1alpha1::{
        DeleteRequest, HealthRequest, QueryRequest, ReadRequest, RequestContext, UpsertRequest,
    };
    use kelvin_memory_api::{
        DelegationClaims, MemoryModuleManifest, MemoryOperation, RequestLimits, JWT_ALGORITHM,
    };

    use crate::config::MemoryControllerConfig;
    use crate::controller::{now_secs, MemoryController};
    use crate::provider::ProviderRegistry;

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

    fn sample_manifest(required_host_features: Vec<String>) -> MemoryModuleManifest {
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

    fn busy_loop_wasm() -> Vec<u8> {
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

    fn claims(jti: &str, operation: MemoryOperation) -> DelegationClaims {
        claims_with_caps(
            jti,
            operation,
            vec![
                "memory_crud".to_string(),
                "memory_read".to_string(),
                "memory_health".to_string(),
            ],
        )
    }

    fn claims_with_caps(
        jti: &str,
        operation: MemoryOperation,
        allowed_capabilities: Vec<String>,
    ) -> DelegationClaims {
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
            allowed_capabilities,
            request_limits: RequestLimits {
                timeout_ms: 300,
                max_bytes: 1024,
                max_results: 5,
            },
        }
    }

    fn mint_context(jti: &str, request_id: &str, operation: MemoryOperation) -> RequestContext {
        let claims = claims(jti, operation);
        let private_key = test_private_key_pem();
        let key = EncodingKey::from_ed_pem(private_key.as_bytes()).expect("encoding");
        let token =
            jsonwebtoken::encode(&Header::new(JWT_ALGORITHM), &claims, &key).expect("encode token");
        RequestContext {
            delegation_token: token,
            request_id: request_id.to_string(),
            tenant_id: "tenant-a".to_string(),
            workspace_id: "workspace-a".to_string(),
            session_id: "session-a".to_string(),
            module_id: "memory.echo".to_string(),
        }
    }

    async fn controller_with_module(wasm: Vec<u8>) -> Arc<MemoryController> {
        let mut config = MemoryControllerConfig::default();
        config.decoding_key_pem = test_public_key_pem();
        config.default_timeout_ms = 150;
        config.default_fuel = 5_000;
        let controller = Arc::new(
            MemoryController::new(config, ProviderRegistry::with_default_in_memory())
                .expect("controller"),
        );
        controller
            .register_module_bytes(sample_manifest(vec!["provider_sqlite".to_string()]), &wasm)
            .await
            .expect("register module");
        controller
    }

    #[tokio::test]
    async fn crud_query_health_happy_path() {
        let controller = controller_with_module(sample_wasm()).await;

        let upsert = controller
            .upsert(Request::new(UpsertRequest {
                context: Some(mint_context(
                    "jti-upsert-1",
                    "req-1",
                    MemoryOperation::Upsert,
                )),
                key: "MEMORY.md".to_string(),
                value: b"router on vlan10".to_vec(),
                metadata: Default::default(),
            }))
            .await
            .expect("upsert")
            .into_inner();
        assert!(upsert.stored);

        let query = controller
            .query(Request::new(QueryRequest {
                context: Some(mint_context("jti-query-1", "req-2", MemoryOperation::Query)),
                query: "router".to_string(),
                max_results: 5,
            }))
            .await
            .expect("query")
            .into_inner();
        assert_eq!(query.hits.len(), 1);

        let read = controller
            .read(Request::new(ReadRequest {
                context: Some(mint_context("jti-read-1", "req-3", MemoryOperation::Read)),
                key: "MEMORY.md".to_string(),
            }))
            .await
            .expect("read")
            .into_inner();
        assert!(read.found);

        let health = controller
            .health(Request::new(HealthRequest {
                context: Some(mint_context(
                    "jti-health-1",
                    "req-4",
                    MemoryOperation::Health,
                )),
            }))
            .await
            .expect("health")
            .into_inner();
        assert!(health.ok);
    }

    #[tokio::test]
    async fn rejects_replayed_token_jti() {
        let controller = controller_with_module(sample_wasm()).await;
        let request = UpsertRequest {
            context: Some(mint_context(
                "jti-replay",
                "req-replay-1",
                MemoryOperation::Upsert,
            )),
            key: "a".to_string(),
            value: b"b".to_vec(),
            metadata: Default::default(),
        };

        controller
            .upsert(Request::new(request.clone()))
            .await
            .expect("first request");
        let second = controller.upsert(Request::new(request)).await;
        assert!(second.is_err());
        assert!(second
            .err()
            .expect("status")
            .message()
            .contains("replayed delegation token"));
    }

    #[tokio::test]
    async fn rejects_context_claim_mismatch() {
        let controller = controller_with_module(sample_wasm()).await;
        let mut context = mint_context("jti-mismatch", "req-ctx-mismatch", MemoryOperation::Read);
        context.workspace_id = "workspace-bad".to_string();
        let result = controller
            .read(Request::new(ReadRequest {
                context: Some(context),
                key: "a".to_string(),
            }))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rejects_operation_not_granted() {
        let controller = controller_with_module(sample_wasm()).await;
        let result = controller
            .delete(Request::new(DeleteRequest {
                context: Some(mint_context(
                    "jti-op-denied",
                    "req-op-denied",
                    MemoryOperation::Read,
                )),
                key: "a".to_string(),
            }))
            .await;
        assert!(result.is_err());
        assert!(result
            .err()
            .expect("status")
            .message()
            .contains("does not allow operation"));
    }

    #[tokio::test]
    async fn rejects_oversized_payload() {
        let controller = controller_with_module(sample_wasm()).await;
        let result = controller
            .upsert(Request::new(UpsertRequest {
                context: Some(mint_context("jti-big", "req-big", MemoryOperation::Upsert)),
                key: "big".to_string(),
                value: vec![1_u8; 2048],
                metadata: Default::default(),
            }))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn idempotency_returns_cached_response_with_new_token() {
        let controller = controller_with_module(sample_wasm()).await;

        let first = controller
            .upsert(Request::new(UpsertRequest {
                context: Some(mint_context(
                    "jti-cache-1",
                    "req-cache",
                    MemoryOperation::Upsert,
                )),
                key: "k".to_string(),
                value: b"v".to_vec(),
                metadata: Default::default(),
            }))
            .await
            .expect("first")
            .into_inner();
        assert!(first.stored);

        let second = controller
            .upsert(Request::new(UpsertRequest {
                context: Some(mint_context(
                    "jti-cache-2",
                    "req-cache",
                    MemoryOperation::Upsert,
                )),
                key: "k".to_string(),
                value: b"v".to_vec(),
                metadata: Default::default(),
            }))
            .await
            .expect("second")
            .into_inner();
        assert!(second.stored);
    }

    #[tokio::test]
    async fn module_fuel_exhaustion_is_rejected() {
        let controller = controller_with_module(busy_loop_wasm()).await;
        let result = controller
            .upsert(Request::new(UpsertRequest {
                context: Some(mint_context(
                    "jti-timeout",
                    "req-timeout",
                    MemoryOperation::Upsert,
                )),
                key: "k".to_string(),
                value: b"v".to_vec(),
                metadata: Default::default(),
            }))
            .await;
        assert!(result.is_err());
        let msg = result.err().expect("status").message().to_string();
        assert!(msg.contains("timed out") || msg.contains("trap") || msg.contains("fuel"));
    }

    #[tokio::test]
    async fn module_registration_rejects_missing_provider_feature() {
        let mut config = MemoryControllerConfig::default();
        config.decoding_key_pem = test_public_key_pem();
        let controller = MemoryController::new(config, ProviderRegistry::with_default_in_memory())
            .expect("controller");

        let err = controller
            .register_module_bytes(
                sample_manifest(vec!["provider_vector_nvidia".to_string()]),
                &sample_wasm(),
            )
            .await
            .expect_err("missing feature should fail");
        assert!(err
            .to_string()
            .contains("requires unavailable host feature"));
    }

    #[tokio::test]
    async fn token_with_wrong_audience_is_rejected() {
        let controller = controller_with_module(sample_wasm()).await;
        let bad_claims = DelegationClaims {
            aud: "wrong-audience".to_string(),
            ..claims("jti-wrong-aud", MemoryOperation::Read)
        };
        let private_key = test_private_key_pem();
        let key = EncodingKey::from_ed_pem(private_key.as_bytes()).expect("encoding");
        let token =
            jsonwebtoken::encode(&Header::new(JWT_ALGORITHM), &bad_claims, &key).expect("token");
        let result = controller
            .read(Request::new(ReadRequest {
                context: Some(RequestContext {
                    delegation_token: token,
                    request_id: "req-aud".to_string(),
                    tenant_id: "tenant-a".to_string(),
                    workspace_id: "workspace-a".to_string(),
                    session_id: "session-a".to_string(),
                    module_id: "memory.echo".to_string(),
                }),
                key: "k".to_string(),
            }))
            .await;
        assert!(result.is_err());
    }

    #[cfg(not(feature = "provider_vector_nvidia"))]
    #[test]
    fn profile_config_validation_rejects_mismatched_build() {
        let mut cfg = MemoryControllerConfig::default();
        cfg.decoding_key_pem = test_public_key_pem();
        cfg.profile = crate::config::ProviderProfile::LinuxGpu;
        let result = MemoryController::new(cfg, ProviderRegistry::with_default_in_memory());
        assert!(result.is_err());
        assert!(result
            .err()
            .expect("error")
            .to_string()
            .contains("requires provider_vector_nvidia"));
    }

    #[tokio::test]
    async fn expired_token_is_rejected() {
        let controller = controller_with_module(sample_wasm()).await;
        let now = now_secs();
        let expired = DelegationClaims {
            exp: now.saturating_sub(120),
            nbf: now.saturating_sub(180),
            ..claims("jti-expired", MemoryOperation::Read)
        };
        let private_key = test_private_key_pem();
        let key = EncodingKey::from_ed_pem(private_key.as_bytes()).expect("encoding");
        let token =
            jsonwebtoken::encode(&Header::new(JWT_ALGORITHM), &expired, &key).expect("token");
        let result = controller
            .read(Request::new(ReadRequest {
                context: Some(RequestContext {
                    delegation_token: token,
                    request_id: "req-expired".to_string(),
                    tenant_id: "tenant-a".to_string(),
                    workspace_id: "workspace-a".to_string(),
                    session_id: "session-a".to_string(),
                    module_id: "memory.echo".to_string(),
                }),
                key: "k".to_string(),
            }))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn denied_capability_is_rejected() {
        let controller = controller_with_module(sample_wasm()).await;
        let restricted = claims_with_caps(
            "jti-no-read-cap",
            MemoryOperation::Read,
            vec!["memory_crud".to_string()],
        );
        let private_key = test_private_key_pem();
        let key = EncodingKey::from_ed_pem(private_key.as_bytes()).expect("encoding");
        let token =
            jsonwebtoken::encode(&Header::new(JWT_ALGORITHM), &restricted, &key).expect("token");
        let result = controller
            .read(Request::new(ReadRequest {
                context: Some(RequestContext {
                    delegation_token: token,
                    request_id: "req-no-read-cap".to_string(),
                    tenant_id: "tenant-a".to_string(),
                    workspace_id: "workspace-a".to_string(),
                    session_id: "session-a".to_string(),
                    module_id: "memory.echo".to_string(),
                }),
                key: "k".to_string(),
            }))
            .await;
        assert!(result.is_err());
        assert!(result
            .err()
            .expect("status")
            .message()
            .contains("missing capability"));
    }

    #[tokio::test]
    async fn query_max_results_over_claim_limit_is_rejected() {
        let controller = controller_with_module(sample_wasm()).await;
        let result = controller
            .query(Request::new(QueryRequest {
                context: Some(mint_context(
                    "jti-too-many-results",
                    "req-too-many-results",
                    MemoryOperation::Query,
                )),
                query: "router".to_string(),
                max_results: 999,
            }))
            .await;
        assert!(result.is_err());
        assert!(result
            .err()
            .expect("status")
            .message()
            .contains("exceeds limit"));
    }
}
