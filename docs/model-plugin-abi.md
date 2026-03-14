# Model Plugin ABI (`wasm_model_v1`)

This document defines the v1 ABI and runtime contract for installed WASM model-provider plugins.

## Runtime Kind

- Manifest `runtime` must be `wasm_model_v1`.
- Manifest `capabilities` must include `model_provider`.
- Manifest must include:
  - `entrypoint`
  - `provider_name` or `provider_profile`
  - `model_name`
  - `capability_scopes.network_allow_hosts` (non-empty for model runtime)

Recommended manifest fields for new plugins:

- `provider_profile`: host-enforced profile id (`openai.responses`, `anthropic.messages`)
- `provider_name`: optional when it matches the selected profile

## Guest Exports

The WASM guest module must export:

- `alloc(len: i32) -> i32`
- `dealloc(ptr: i32, len: i32)`
- `infer(req_ptr: i32, req_len: i32) -> i64`
- linear memory export `memory`

`infer` receives UTF-8 JSON `ModelInput` and returns packed `ptr/len` (`i64`) for UTF-8 JSON output.

## Host Imports

The runtime only allows imports from module `kelvin_model_host_v1`:

- `provider_profile_call(req_ptr: i32, req_len: i32) -> i64`
- `openai_responses_call(req_ptr: i32, req_len: i32) -> i64`
- `log(level: i32, msg_ptr: i32, msg_len: i32) -> i32`
- `clock_now_ms() -> i64`

Any other import module or symbol is rejected at load time.

## JSON Payloads

Input payload is `kelvin_core::ModelInput` JSON:

- `run_id`
- `session_id`
- `system_prompt`
- `user_prompt`
- `memory_snippets`
- `history`

Success output payload is `kelvin_core::ModelOutput` JSON:

- `assistant_text`
- `stop_reason`
- `tool_calls`
- `usage`

Plugin failure payload format:

```json
{
  "error": {
    "message": "provider-specific failure reason"
  }
}
```

The loader maps this to a typed backend failure (fail-closed).

## Runtime Controls

`WasmModelHost` enforces:

- module size limit (`max_module_bytes`)
- request size limit (`max_request_bytes`)
- response size limit (`max_response_bytes`)
- fuel budget (`fuel_budget`)
- timeout (`timeout_ms`)
- network host allowlist (manifest scope intersected with host policy)

The host executes HTTPS calls; guest code never opens raw sockets.

## Provider Profile Host Call

`provider_profile_call` is implemented by the trusted host and resolves the outbound provider from the plugin manifest's `provider_profile`.

Built-in host-enforced profiles:

- `openai.responses`
  - endpoint: `POST /v1/responses`
  - key source: `OPENAI_API_KEY`
  - base URL override: `OPENAI_BASE_URL`
  - default allowlist host: `api.openai.com`
- `anthropic.messages`
  - endpoint: `POST /v1/messages`
  - key source: `ANTHROPIC_API_KEY`
  - base URL override: `ANTHROPIC_BASE_URL`
  - default allowlist host: `api.anthropic.com`
  - fixed header: `anthropic-version: 2023-06-01`

The guest passes provider-specific JSON request payloads; the host supplies auth, endpoint selection, and host allowlist enforcement.

## Legacy OpenAI Host Call

`openai_responses_call` remains available for legacy `wasm_model_v1` plugins:

- endpoint: `POST /v1/responses`
- key source: `OPENAI_API_KEY` (required)
- base URL override: `OPENAI_BASE_URL` (optional, still allowlist-checked)
- default allowlist host: `api.openai.com`

Secrets are not emitted in logs/errors.

## Compatibility

- ABI module name is versioned: `kelvin_model_host_v1`.
- Breaking ABI changes require a new runtime/ABI version (`..._v2`) and side-by-side support during migration.
