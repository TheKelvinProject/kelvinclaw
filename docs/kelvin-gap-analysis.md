# KelvinClaw High-Level Gap Analysis

This document tracks high-level parity gaps and closure work against the reference "Claw" product shape, while preserving KelvinClaw's security-first SDK and control/data plane separation.

## Completed Gap Closures

### 1) Secure Gateway Control Plane

Status: `DONE`

Implemented:

- new app: `apps/kelvin-gateway`
- typed WebSocket request/response/event envelopes
- strict connect-first handshake
- optional auth token enforcement on connect (`KELVIN_GATEWAY_TOKEN` / `--token`)
- idempotent `agent` submission via required `request_id`
- async run surfaces:
  - `agent` / `run.submit`
  - `agent.wait` / `run.wait`
  - `agent.state` / `run.state`
  - `agent.outcome` / `run.outcome`
- streamed runtime events from SDK runtime to connected clients

Security properties:

- fail-closed handshake validation
- explicit auth check before runtime operations
- method allowlist and typed parameter validation
- no direct plugin loading in gateway code (SDK-only composition path)

### 2) Model Failover + Retry Semantics

Status: `DONE`

Implemented in `kelvin-sdk`:

- `KelvinSdkModelSelection::InstalledPluginFailover`
- ordered provider chain selection
- bounded retries per provider (`max_retries_per_provider`)
- bounded backoff (`retry_backoff_ms`)
- fail-closed behavior:
  - retry/fallback only on transient classes (`backend`, `timeout`, `io`)
  - no fallback on non-recoverable classes (`invalid_input`, `not_found`)

Security and reliability properties:

- no silent fallback to unintended providers
- explicit provider ordering and retry bounds
- deterministic error surfaces when chain is exhausted

### 3) Reusable SDK Runtime for Host/Gateway Surfaces

Status: `DONE`

Implemented:

- `KelvinSdkRuntimeConfig`
- `KelvinSdkRuntime::initialize(...)`
- `KelvinSdkRuntime::submit/state/wait/wait_for_outcome`
- `KelvinSdkRunRequest` + `KelvinSdkAcceptedRun`

Architecture impact:

- external surfaces can now use the SDK runtime directly instead of composing root crates.
- host and gateway stay on the same policy-governed composition path.

## Remaining High-Level Gaps

These are still open and are prioritized by security, stability, and maintainability impact.

### 1) Channel Integrations

Status: `DONE`

Needed:

- production channel adapters (chat/voice surfaces)
- per-channel auth/routing/allowlist policy
- deterministic delivery/retry + rate controls per channel

Now implemented:

- Telegram ingress lane with dedupe, pairing, retry, and rate limiting
- Slack ingress lane with ingress auth token enforcement, per-sender trust tiers, quotas, and dedupe
- Discord ingress lane with ingress auth token enforcement, per-sender trust tiers, quotas, and dedupe
- channel status observability metrics (ingest/dedupe/pairing/rate/timeout/retry/failure counters)
- channel conformance integration tests for ordering/idempotency/auth mismatch/flood handling
- optional per-channel WASM ingress policy plugin ABI (`kelvin_channel_host_v1`)

### 2) Daemon Lifecycle + Operator UX

Status: `PARTIAL`

Needed:

- first-class daemon install/start/stop/status
- startup health checks and fail-fast diagnostics
- remote-safe defaults for exposure/auth

Now implemented:

- `scripts/kelvin-local-profile.sh` for local background memory+gateway lifecycle
- actionable machine-readable doctor checks with remediation hints
- canonical quickstart flow (`scripts/quickstart.sh`)

### 3) Control UI and Operator Observability

Status: `PARTIAL`

Needed:

- minimal web/operator UI over gateway APIs
- run/session/event inspection
- policy and plugin state visibility

Now implemented:

- operator console served from the ingress listener (`/operator/`) with:
  - gateway overview and security warnings
  - channel ingress/delivery state
  - scheduler list/history views
  - run submission and run inspection
- gateway health payload and channel status surfaces

### 4) Rich Context Management (Compaction/Pruning)

Status: `DONE`

Needed:

- deterministic compaction policy
- pruning thresholds + summaries
- run-level bounds on context growth

Now implemented:

- configurable compaction controls (`max_session_history_messages`, `compact_to_messages`)
- persisted run/session state with bounded history and compacted summaries
- corrupt session-state recovery with quarantine behavior

### 5) Multi-provider Auth Profiles and Routing Policy

Status: `DONE`

Needed:

- credential profile abstraction
- policy-based model/provider routing
- typed fallback trees tied to workspace/session policy

Now implemented for channel lane:

- deterministic channel routing rules (`KELVIN_CHANNEL_ROUTING_RULES_JSON`)
- routing by channel/account/workspace/session with sender trust-tier matching
- route metadata surfaced in channel ingest responses
- explicit route inspection method (`channel.route.inspect`) for operator validation

### 6) Core Value Tools (SDK Plugin Lane)

Status: `DONE`

Implemented:

- first-party Kelvin Core tool-pack plugins in SDK lane:
  - `fs_safe_read`
  - `fs_safe_write`
  - `web_fetch_safe` (strict host allowlist)
  - `schedule_cron`
  - `session_tools`
- explicit sensitive-operation approval payloads (`approval.granted` + `approval.reason`)
- structured tool execution receipts (`who/what/why/result_class/latency_ms`)
- graceful degradation: tool failures no longer crash whole runs
- deterministic OWASP/NIST tool sandbox suites

### 7) Ecosystem and Plugin DX

Status: `PARTIAL`

Implemented:

- plugin author command flow:
  - `kelvin plugin new`
  - `kelvin plugin test`
  - `kelvin plugin pack`
  - `kelvin plugin verify`
- plugin author templates under `templates/plugin-author-kit/`
- plugin quality tier conventions (`unsigned_local`, `signed_community`, `signed_trusted`)
- plugin discovery helper (`scripts/plugin-discovery.sh`)
- trust policy lifecycle ops (`scripts/plugin-trust.sh`) for:
  - key rotation
  - revocation
  - plugin publisher pinning
- runtime trust enforcement for revoked publishers and pin mismatches

Still open:

- fully hosted plugin registry API service endpoints (current discovery is static index + helper)
- multi-version ABI compatibility CI matrix against external plugin repos
- automated publisher key rotation pipelines

## Near-Term TODO (Execution Order)

1. Add hosted plugin registry API service endpoints beyond static index.
2. Add external plugin repository compatibility-matrix CI lanes.
3. Add daemon/service management for `kelvin-gateway` (systemd/launchd docs + scripts).
4. Add gateway security tests for malformed frames, replay pressure, and auth brute-force throttling.
5. Add plugin/trust-policy/operator pages beyond the current console shell.
6. Keep consolidating Docker layer/cargo cache reuse across developer and CI scripts without letting `.cache/docker` grow unchecked.
