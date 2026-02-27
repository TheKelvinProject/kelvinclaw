# Channel Plugin ABI (`kelvin_channel_host_v1`)

Kelvin channel policy plugins are untrusted WASM modules executed by the trusted gateway host.

This ABI is designed for ingress policy decisions and message shaping before channel routing and run submission.

## Security Model

- module execution is sandboxed with fuel budget and payload bounds
- only explicit host imports are available
- no direct filesystem/network/shell access from channel WASM
- fail-closed behavior: invalid output or runtime errors reject ingress

## ABI Surface (v1)

Module name:

- `kelvin_channel_host_v1`

Required exports:

- `memory`
- `alloc(len: i32) -> i32`
- `dealloc(ptr: i32, len: i32)`
- `handle_ingest(req_ptr: i32, req_len: i32) -> i64`

Allowed imports:

- `log(level: i32, msg_ptr: i32, msg_len: i32) -> i32`
- `clock_now_ms() -> i64`

`handle_ingest` return value packs output pointer/length:

- upper 32 bits: pointer
- lower 32 bits: length

## Host Input JSON

`handle_ingest` receives UTF-8 JSON with this shape:

- `channel`: `telegram | slack | discord`
- `delivery_id`: string
- `sender_id`: string
- `account_id`: string
- `text`: string
- `timeout_ms`: number or null
- `session_id`: string or null
- `workspace_dir`: string or null
- `trust_tier`: `trusted | standard | probation | blocked`
- `now_ms`: unix epoch milliseconds

## Guest Output JSON

Guest must return UTF-8 JSON with this shape:

- `allow`: boolean (required)
- `reason`: string (optional, used on deny)
- `trust_tier`: `trusted | standard | probation | blocked` (optional override)
- `override_text`: string (optional)
- `route_session_id`: string (optional)
- `route_workspace_dir`: string (optional)
- `route_system_prompt`: string (optional)

If `allow=false`, ingress is rejected.

## Configuration

Per channel:

- `KELVIN_<CHANNEL>_WASM_POLICY_PATH`
- `KELVIN_<CHANNEL>_WASM_MAX_MODULE_BYTES`
- `KELVIN_<CHANNEL>_WASM_MAX_REQUEST_BYTES`
- `KELVIN_<CHANNEL>_WASM_MAX_RESPONSE_BYTES`
- `KELVIN_<CHANNEL>_WASM_FUEL_BUDGET`

`<CHANNEL>` is `TELEGRAM`, `SLACK`, or `DISCORD`.

## Stability

- ABI version is currently `1.0.0`
- additive-compatible evolution only for v1
- host rejects unsupported import modules/functions
