---
issue: Closes #2272
shares: #2295 both add a match arm in crates/acp/src/update.rs; this adds one arm for the `Other` event case
shares: #2294 both add a match arm in crates/acp/src/update.rs; this adds one arm for the `Other` event case
shares: #2239 both touch crates/acp/src/update.rs and crates/console/src/settings.rs; this adds one arm in each for the new `Other` case
shares: #2256 touches crates/types/tests/settings.rs; this adds one `SettingKind::Other` arm to its sample helper
raise: crate context +6, crate runtime +3, crate types +123, tests +75
---
Agent events, agent messages, child activities and setting kinds gain an `Other` case. A tag or role this build does not know decodes to it and re-emits byte for byte, so an older Yi reads a newer one's events without a typed failure. A known tag whose body does not decode is still an error, not `Other`. `ChildActivity` drops `Copy` because an unknown activity is a string. The context, runtime and TUI paths show an unknown message as a notice, and an unknown setting kind shows no value.
