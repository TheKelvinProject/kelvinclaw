# Memory Controller Deployment Profiles

## Build Profiles

Controller exposes feature-based provider profiles:

- `profile_minimal`: `provider_sqlite` (+ in-memory runtime provider)
- `profile_iphone`: `provider_sqlite`, `provider_object_store`, `provider_vector_metal`
- `profile_linux_gpu`: `provider_sqlite`, `provider_object_store`, `provider_vector_nvidia`

Examples:

```bash
cargo build -p kelvin-memory-controller --no-default-features --features profile_minimal
cargo build -p kelvin-memory-controller --no-default-features --features profile_iphone
cargo build -p kelvin-memory-controller --no-default-features --features profile_linux_gpu
```

## Runtime Configuration

Controller environment:

- `KELVIN_MEMORY_CONTROLLER_ADDR`
- `KELVIN_MEMORY_PUBLIC_KEY_PEM`
- `KELVIN_MEMORY_PUBLIC_KEY_PATH`
- `KELVIN_MEMORY_ISSUER`
- `KELVIN_MEMORY_AUDIENCE`
- `KELVIN_MEMORY_PROFILE`
- `KELVIN_MEMORY_CLOCK_SKEW_SECS`
- `KELVIN_MEMORY_REPLAY_WINDOW_SECS`
- `KELVIN_MEMORY_DEFAULT_TIMEOUT_MS`
- `KELVIN_MEMORY_DEFAULT_FUEL`
- `KELVIN_MEMORY_MAX_MODULE_BYTES`
- `KELVIN_MEMORY_MAX_MEMORY_PAGES`
- `KELVIN_MEMORY_DEFAULT_MAX_RESPONSE_BYTES`
- `KELVIN_MEMORY_ALLOW_INSECURE_NON_LOOPBACK`
- `KELVIN_MEMORY_TLS_CERT_PEM` or `KELVIN_MEMORY_TLS_CERT_PATH`
- `KELVIN_MEMORY_TLS_KEY_PEM` or `KELVIN_MEMORY_TLS_KEY_PATH`
- `KELVIN_MEMORY_TLS_CLIENT_CA_PEM` or `KELVIN_MEMORY_TLS_CLIENT_CA_PATH` (optional mTLS)

Root-side client signing can use:

- `KELVIN_MEMORY_SIGNING_KEY_PEM` or `KELVIN_MEMORY_SIGNING_KEY_PATH`
- `KELVIN_MEMORY_SIGNING_KMS_KEY_ID` with optional `KELVIN_MEMORY_SIGNING_KMS_REGION`

GitHub Actions validation can use the Blacksmith-backed workflow
`.github/workflows/memory-kms-smoke.yml`, which assumes
`arn:aws:iam::REDACTED_ACCOUNT_ID:role/REDACTED_MEMORY_ROLE_NAME` via GitHub OIDC.

The controller does not call KMS directly; it verifies against the exported public
key PEM configured above.

Network safety default:

- Controller refuses non-loopback plaintext binds unless either:
- TLS cert/key are configured, or
- `KELVIN_MEMORY_ALLOW_INSECURE_NON_LOOPBACK=true` is explicitly set.
- Use insecure override only behind a trusted boundary (private VPC + service ACLs) and prefer TLS/mTLS.

## Profile Guarantees

- iPhone profile excludes NVIDIA vector feature.
- Linux GPU profile includes NVIDIA vector feature.
- minimal profile stays small and excludes GPU-specialized providers.

## Module Admission

Module registration fails fast when `required_host_features` are unavailable in the current build profile.

## Operations

Runbooks:

- `docs/runbooks/memory-jwt-key-rotation.md`
- `docs/runbooks/module-publisher-trust-policy.md`
- `docs/runbooks/memory-module-denial-timeout-storms.md`
