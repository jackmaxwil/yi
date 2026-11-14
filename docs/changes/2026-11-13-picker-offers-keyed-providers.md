---
issue: Closes #2204
decision: The console and TUI model pickers offer only providers that hold a credential | An unkeyed provider's models were offered beside the ones the user can run, and picking one only failed at its first turn; with no credential anywhere the whole catalog is still offered so a fresh install can browse | Call the unfiltered registry list in both pickers again
raise: crate ai +4, crate runtime +73, tests +97
---

The model pickers now list only providers with a credential: an environment variable, a configured custom key, or a stored login. The check reads the store without refreshing a token, so opening the picker makes no network call. The session's current model stays listed even when its provider has no credential, and when no provider has one the picker falls back to the whole catalog. The RPC `get_available_models` list is unchanged.
