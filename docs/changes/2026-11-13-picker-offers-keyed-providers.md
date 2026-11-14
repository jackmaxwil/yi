---
issue: Closes #2204
decision: The console and TUI model pickers offer only providers that hold a credential | An unkeyed provider's models were offered beside the ones the user can run, and picking one only failed at its first turn; with no credential anywhere the whole catalog is still offered so a fresh install can browse | Call the unfiltered registry list in both pickers again
raise: crate ai +4, crate runtime +73, tests +97
cut: crates/ai/src/kimi.rs | let context_window = positive("context").ok_or("no positive limit.context")?; | the stacked review commit moved the limit parsing out of kimi.rs into the shared DiscoveredCustomLimit reader, one rule for the limit object
cut: crates/cli/tests/custom_providers.rs | fn fake_gateway() -> std::io::Result<(u16, Seen)> { | the stacked review commits replaced it with fake_gateway_serving(discovery) and moved the rpc child into the shared run_rpc helper
shares: #2167 two lines of crates/acp/src/lib.rs (the picker's options call and its import) fall in different hunks than #2167's daemon work and the two branches merge cleanly, and the integration.rs overlap is a two-line module registration from the stacked #2203 commits
---

The model pickers now list only providers with a credential: an environment variable, a configured custom key, or a stored login. The check reads the store without refreshing a token, so opening the picker makes no network call. The session's current model stays listed even when its provider has no credential, and when no provider has one the picker falls back to the whole catalog. The RPC `get_available_models` list is unchanged.
