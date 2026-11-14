---
issue: Refs #2201
raise: crate types +15
---

Discovery now reads the output limit a provider advertises. It accepts `max_tokens`, then `max_output_tokens`, then `limit.output`, and falls back to `limit.context` for the context window. Before, a relay that advertised only `max_output_tokens` and `limit` (as OpenRelay does) had every discovered model capped at the 16,384 default.
