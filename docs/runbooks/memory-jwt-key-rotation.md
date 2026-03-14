# Runbook: JWT Signing Key Rotation

## Purpose

Rotate Root signing keys used for memory delegation JWTs without downtime.

## Procedure

1. Create or rotate the Ed25519 signer in AWS KMS.
2. Export the new public key PEM from KMS and stage it into controller config (`KELVIN_MEMORY_PUBLIC_KEY_PEM` or `KELVIN_MEMORY_PUBLIC_KEY_PATH`).
3. Deploy controller with the staged verifier and acceptance window for both old/new issuers if needed.
4. Update Root to mint tokens with `KELVIN_MEMORY_SIGNING_KMS_KEY_ID` and optional `KELVIN_MEMORY_SIGNING_KMS_REGION`.
5. Observe memory RPC success/error rates and token verification failures.
6. Remove old public key acceptance after the steady-state window.
7. Disable and schedule deletion for the old KMS key, or securely destroy legacy offline key material after migration.

## Validation

- controller accepts new tokens.
- controller rejects old tokens after cutover window.
- no replay cache explosion or authz bypass during transition.
