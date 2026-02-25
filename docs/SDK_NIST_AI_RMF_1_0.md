# SDK NIST AI RMF Coverage

This document maps the Kelvin Core SDK test suite to the NIST AI Risk Management Framework (AI RMF 1.0).

Sources:

- NIST AI RMF 1.0 (PDF): <https://nvlpubs.nist.gov/nistpubs/ai/NIST.AI.100-1.pdf>
- NIST AI RMF landing page: <https://www.nist.gov/itl/ai-risk-management-framework>

## Core Function Mapping

Test suite:

- `crates/kelvin-core/tests/sdk_nist_ai_rmf_1_0.rs`

### GOVERN

- `govern_default_policy_denies_privileged_and_experimental_capabilities`
- `govern_explicit_policy_can_allow_documented_privileges`

Focus:

- default-deny admission policy
- explicit, auditable privilege opt-in

### MAP

- `map_manifest_rejects_untraceable_metadata_shapes`
- `map_tool_provider_capability_must_match_actual_tool_export`
- `map_declared_tool_provider_requires_concrete_tool`

Focus:

- well-formed plugin metadata
- clear capability-to-implementation boundaries

### MEASURE

- `measure_compatibility_report_is_actionable_and_specific`
- `measure_tool_projection_order_is_deterministic`
- `measure_duplicate_capabilities_are_rejected_for_clean_metrics`

Focus:

- reproducible compatibility signals
- deterministic behavior
- low-ambiguity capability declarations

### MANAGE

- `manage_duplicate_registration_prevents_state_corruption`
- `manage_unknown_lookup_is_safe_and_non_panicking`
- `manage_concurrent_duplicate_registration_allows_one_winner`
- `manage_large_registry_projection_remains_stable`

Focus:

- resilient failure handling
- safe fallback behavior
- concurrency stability under contention

## Scope Notes

- This is SDK-lane coverage, not full application/runtime governance.
- Root-lane integrations remain outside SDK safety guarantees by design (`docs/ROOT_VS_SDK.md`).
