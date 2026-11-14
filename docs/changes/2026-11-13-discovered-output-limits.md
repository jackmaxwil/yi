---
version: 0.1048.0
issue: Refs #2201
cut: crates/types/src/config.rs | provider.models = (list.data.into_iter()) | the filter/map chain became one filter_map that resolves the limit fallbacks before validating
raise: crate types +52, tests +109
---

Discovery now reads the output limit a provider advertises. It accepts `max_tokens`, then `max_output_tokens`, then `limit.output`, and falls back to `limit.context` for the context window. Before, a relay that advertised only `max_output_tokens` and `limit` (as OpenRelay does) had every discovered model capped at the 16,384 default.
