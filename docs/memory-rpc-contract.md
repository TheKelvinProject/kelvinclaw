# Memory RPC Contract (`v1alpha1`)

## Service

`kelvin.memory.v1alpha1.MemoryService`

Unary RPCs:

- `Upsert(UpsertRequest) -> UpsertResponse`
- `Query(QueryRequest) -> QueryResponse`
- `Read(ReadRequest) -> ReadResponse`
- `Delete(DeleteRequest) -> DeleteResponse`
- `Health(HealthRequest) -> HealthResponse`

Source: `crates/kelvin-memory-api/proto/kelvin/memory/v1alpha1/memory.proto`

## Transport Security

- Local/dev defaults use `http://127.0.0.1:50051`.
- Production is expected to use `https://...` with TLS configured on controller.
- Optional mTLS can be enabled by configuring controller client-CA and client cert/key on root-side client.
- Root client JWT signing key:
- `KELVIN_MEMORY_SIGNING_KEY_PEM` or `KELVIN_MEMORY_SIGNING_KEY_PATH`
- or `KELVIN_MEMORY_SIGNING_KMS_KEY_ID` with optional `KELVIN_MEMORY_SIGNING_KMS_REGION`
- Root client TLS knobs:
- `KELVIN_MEMORY_RPC_TLS_CA_PEM` or `KELVIN_MEMORY_RPC_TLS_CA_PATH`
- `KELVIN_MEMORY_RPC_TLS_DOMAIN_NAME`
- `KELVIN_MEMORY_RPC_TLS_CLIENT_CERT_PEM` or `KELVIN_MEMORY_RPC_TLS_CLIENT_CERT_PATH`
- `KELVIN_MEMORY_RPC_TLS_CLIENT_KEY_PEM` or `KELVIN_MEMORY_RPC_TLS_CLIENT_KEY_PATH`

For KMS-backed signing, controller verification still uses an exported public key via
`KELVIN_MEMORY_PUBLIC_KEY_PEM` or `KELVIN_MEMORY_PUBLIC_KEY_PATH`.

## Required Context

Every request includes `RequestContext`:

- `delegation_token` (JWT)
- `request_id` (idempotency key)
- `tenant_id`
- `workspace_id`
- `session_id`
- `module_id`

Controller requires strict equality between context fields and token claims.

## Delegation Claims

Signed JWT claims include:

- core: `iss`, `sub`, `aud`, `jti`, `exp`, `nbf`
- tenancy: `tenant_id`, `workspace_id`, `session_id`
- module scope: `module_id`, `allowed_ops`, `allowed_capabilities`
- limits: `request_limits.timeout_ms`, `request_limits.max_bytes`, `request_limits.max_results`

## Idempotency

`request_id` is used for response deduplication in controller cache. Replays by JWT `jti` are denied.

## Compatibility Rules

- package and service name are fixed for `v1alpha1`.
- request/response field numbers are append-only.
- required compatibility checks run from `kelvin-memory-api` descriptor tests.

## Error Mapping

Controller maps to gRPC status codes:

- `InvalidInput -> INVALID_ARGUMENT`
- `NotFound -> NOT_FOUND`
- `Timeout -> DEADLINE_EXCEEDED`
- `Backend -> UNAVAILABLE`

Client maps these to `KelvinError` categories for root callers.
