---
issue: Refs #2204
raise: crate runtime +1, tests +12
cut: crates/runtime/src/discover.rs | fn keep_usable(models: Vec<Model>, current: &Model, usable: impl Fn(&str) -> bool) -> Vec<Model> { | the picker filter now lives once in subagent::models::usable_models, shared with the subagent model list
cut: crates/cli/tests/custom_providers.rs | fn a_model_list_with_no_usable_model_is_named() -> TestResult { | a list whose limits are all zero now falls back to catalog defaults, covered by a_model_list_with_zero_limits_uses_catalog_defaults
---

The picker credential filter now shares one rule between the console and TUI pickers and the subagent model list, and a discovered model's `max_output_tokens` no longer fails the whole model list when a gateway sends it as a non-number.
