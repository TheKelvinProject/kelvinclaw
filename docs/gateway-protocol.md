# Kelvin Gateway Protocol (v1)

`apps/kelvin-gateway` exposes a WebSocket control plane over Kelvin SDK runtime composition.

Protocol version constant: `1.0.0`.

## Security Defaults

- connect-first handshake is required
- optional token auth on connect (`KELVIN_GATEWAY_TOKEN` or `--token`)
- typed request validation (`deny_unknown_fields`)
- fail-closed unknown method handling
- idempotency cache for side-effecting `agent` requests via required `request_id`
- channel adapters are disabled unless explicitly enabled by environment config
- Telegram channel defaults to pairing-required and host allowlist checks
- Slack/Discord channels are available behind explicit env enable flags
- optional per-channel WASM policy plugin (`kelvin_channel_host_v1`) can deny/shape ingress before routing

## Envelope

Client request:

```json
{
  "type": "req",
  "id": "req-1",
  "method": "connect",
  "params": {}
}
```

Server response:

```json
{
  "type": "res",
  "id": "req-1",
  "ok": true,
  "payload": {}
}
```

Server event:

```json
{
  "type": "event",
  "event": "agent",
  "payload": {}
}
```

## Handshake

First frame must be `connect`.

`connect` params:

- `auth.token` (required when gateway token is configured)
- `client_id` (optional)

Successful `connect` responses include:

- `protocol_version`
- `supported_methods`

## Methods

- `health`
- `agent` (alias: `run.submit`)
  - params: `request_id`, `prompt`, optional `session_id`, `workspace_dir`, `timeout_ms`, `system_prompt`, `memory_query`, `run_id`
- `agent.wait` (alias: `run.wait`)
  - params: `run_id`, optional `timeout_ms`
- `agent.state` (alias: `run.state`)
  - params: `run_id`
- `agent.outcome` (alias: `run.outcome`)
  - params: `run_id`, optional `timeout_ms`
- `channel.telegram.ingest`
  - params: `delivery_id`, `chat_id`, `text`, optional `timeout_ms`, `auth_token`, `session_id`, `workspace_dir`
- `channel.telegram.pair.approve`
  - params: `code`
- `channel.telegram.status`
  - params: none
- `channel.slack.ingest`
  - params: `delivery_id`, `channel_id`, `user_id`, `text`, optional `timeout_ms`, `auth_token`, `session_id`, `workspace_dir`
- `channel.slack.status`
  - params: none
- `channel.discord.ingest`
  - params: `delivery_id`, `channel_id`, `user_id`, `text`, optional `timeout_ms`, `auth_token`, `session_id`, `workspace_dir`
- `channel.discord.status`
  - params: none
- `channel.route.inspect`
  - params: `channel`, `account_id`, optional `sender_tier`, `session_id`, `workspace_dir`

## Telegram Channel Policy

Telegram channel is configured only via environment variables and remains disabled unless
`KELVIN_TELEGRAM_ENABLED=true`.

- `KELVIN_TELEGRAM_API_BASE_URL` must be `https://api.telegram.org` by default
- custom Telegram base URL requires `KELVIN_TELEGRAM_ALLOW_CUSTOM_BASE_URL=true`
- pairing is enabled by default (`KELVIN_TELEGRAM_PAIRING_ENABLED=true`)
- allowlist is optional (`KELVIN_TELEGRAM_ALLOW_CHAT_IDS=...`)
- inbound dedupe, per-chat rate limits, and bounded retries are always applied

## Slack + Discord Policy

Slack and Discord channels are disabled unless explicitly enabled:

- `KELVIN_SLACK_ENABLED=true`
- `KELVIN_DISCORD_ENABLED=true`

Common policy controls per channel include:

- ingress auth token enforcement (`*_INGRESS_TOKEN`)
- account/sender allowlists and trust tiers (`*_ALLOW_ACCOUNT_IDS`, `*_ALLOW_SENDER_IDS`, `*_TRUSTED_SENDER_IDS`, `*_PROBATION_SENDER_IDS`, `*_BLOCKED_SENDER_IDS`)
- per-tier quotas (`*_MAX_MESSAGES_PER_MINUTE`, `*_MAX_MESSAGES_PER_MINUTE_TRUSTED`, `*_MAX_MESSAGES_PER_MINUTE_PROBATION`)
- probation cooldown (`*_COOLDOWN_MS_PROBATION`)
- bounded inbox + delivery-id dedupe (`*_MAX_QUEUE_DEPTH`, `*_MAX_SEEN_DELIVERY_IDS`)
- bounded outbound retries (`*_OUTBOUND_MAX_RETRIES`, `*_OUTBOUND_RETRY_BACKOFF_MS`)

Default base URL host allowlist is enforced unless explicitly relaxed:

- Slack: `slack.com`
- Discord: `discord.com`

Custom base URLs require `*_ALLOW_CUSTOM_BASE_URL=true`.

## Routing Rules

Channel routing is loaded from `KELVIN_CHANNEL_ROUTING_RULES_JSON` (JSON array).

Each rule supports deterministic matching by:

- `priority` (higher first)
- tie-breaker: rule `id` (lexicographic)

Match fields:

- `channel` (`telegram`, `slack`, `discord`, or `*`)
- optional `account_id`
- optional `sender_tier`
- optional `session_id`
- optional `workspace_dir`

Route output fields:

- `route_session_id`
- `route_workspace_dir`
- `route_system_prompt`

Gateway includes route metadata in channel ingest responses.

## WASM Channel Plugin ABI

Per-channel WASM policy plugin (optional):

- env: `KELVIN_<CHANNEL>_WASM_POLICY_PATH`
- ABI module: `kelvin_channel_host_v1`
- export: `handle_ingest`
- imports: `log`, `clock_now_ms`
- host has no network/system call imports

Reference: [docs/channel-plugin-abi.md](channel-plugin-abi.md)

## Idempotency

`agent` requires `request_id`.

- first submission stores acceptance metadata in the cache
- repeated submission with the same `request_id` returns the cached acceptance and `deduped: true`

## Compatibility Policy

- `protocol_version` is the compatibility anchor for gateway clients.
- method names in `supported_methods` are treated as a frozen v1 surface.
- new methods are additive; existing method names and behavior are preserved for v1 clients.

## Errors

Response envelope uses:

- `ok: false`
- `error.code`
- `error.message`

Typical codes:

- `handshake_required`
- `unauthorized`
- `invalid_input`
- `not_found`
- `timeout`
- `backend_error`
- `io_error`
- `method_not_found`
