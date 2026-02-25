# SDK OWASP Top 10 AI Stress Coverage (2025)

This document maps the Kelvin Core SDK stress suite to the OWASP Top 10 for LLM Applications 2025 risk categories.

Test suite:

- `crates/kelvin-core/tests/sdk_owasp_top10_ai_2025.rs`

## Mapping

- `LLM01: Prompt Injection`
  - `llm01_prompt_injection_rejects_control_characters_in_identity_fields`
  - Ensures control-character payloads are rejected at SDK manifest boundary.

- `LLM02: Sensitive Information Disclosure`
  - `llm02_sensitive_information_disclosure_keeps_error_messages_bounded`
  - Ensures oversized/sensitive payloads are not echoed in full error output.

- `LLM03: Supply Chain`
  - `llm03_supply_chain_rejects_untrusted_version_surface`
  - Ensures semver/API compatibility checks fail closed.

- `LLM04: Data and Model Poisoning`
  - `llm04_data_and_model_poisoning_prevents_plugin_identity_takeover`
  - Ensures duplicate plugin identity cannot overwrite registered plugin.

- `LLM05: Improper Output Handling`
  - `llm05_improper_output_handling_rejects_hidden_tool_exports`
  - Ensures capability declaration matches implementation.

- `LLM06: Excessive Agency`
  - `llm06_excessive_agency_defaults_to_least_privilege`
  - Ensures privileged capabilities (`fs_read`, `fs_write`, `network_egress`, `command_execution`) are denied by default.

- `LLM07: System Prompt Leakage`
  - `llm07_system_prompt_leakage_rejects_control_characters_in_metadata`
  - Ensures metadata fields reject control-character payloads.

- `LLM08: Vector and Embedding Weaknesses`
  - `llm08_vector_and_embedding_weaknesses_isolate_non_tool_plugins_from_tool_projection`
  - Ensures non-tool plugin types cannot silently enter tool execution path.

- `LLM09: Misinformation`
  - `llm09_misinformation_controls_require_deterministic_tool_projection`
  - Ensures deterministic SDK projection order for reproducible behavior.

- `LLM10: Unbounded Consumption`
  - `llm10_unbounded_consumption_stress_registers_large_plugin_sets`
  - Stress validates large plugin registration/projection path without instability.

## Scope Notes

- These are SDK-lane controls, not full runtime/LLM behavior controls.
- Root-lane integrations remain outside SDK trust guarantees by design (`docs/ROOT_VS_SDK.md`).

## Data Plane Extension

Memory Controller OWASP suite:

- `crates/kelvin-memory-controller/tests/memory_controller_owasp_top10_ai_2025.rs`

This adds OWASP-oriented checks for delegation-token misuse, context tampering,
capability overreach, request bounds enforcement, provider-feature gating, and
deterministic query ordering in the memory data plane.
