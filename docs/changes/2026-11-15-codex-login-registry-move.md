---
issue: #2221
raise: tests +55
cut: crates/oauth/src/codex.rs | pub const CODEX_ID: &str = "openai-codex"; | the profile and its id moved into registry.rs beside the Claude profile, so the one-provider module has no reason to exist
---

The built-in Codex profile now lives in the oauth registry beside the Claude one, and `crates/oauth/src/codex.rs` is gone. Reverting the Codex login means deleting `codex_profile` and its `CODEX_ID` lookup arm in `crates/oauth/src/registry.rs`, and the `CODEX_ID` entry in `ids()`. The new `codex_profile.rs` test file pins the profile's identity and `a_refresh_keeps_the_account_id_the_response_does_not_repeat` pins refresh behaviour that already existed.
