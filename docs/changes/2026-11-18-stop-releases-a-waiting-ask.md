---
version: 0.1078.0
issue: Closes #2280, Closes #2291
shares: #2294 both edit acp_unit.rs and runtime/src/lib.rs: this branch adds a stop test and re-exports `Stop`, so the two merge as separate hunks
shares: #2256 both edit runtime/src/lib.rs and session.rs: this branch adds one re-export and one call in `use_tools_with_background`
shares: #2239 both edit acp/src/permission.rs and acp_unit.rs: this branch changes the asker's wait loop and adds a test
shares: #2237 edits runtime/src/lib.rs: this branch adds one name to a `pub use`
shares: #2295 both edit runtime/src/lib.rs and session.rs: this branch adds one re-export and one call in use_tools_with_background
growth: +54 a stop check on the ask, polled by the ACP asker; nothing deletable, the ask had no way to hear an abort
raise: crate acp +7, crate runtime +60, crate types +2, tests +106, comments +3
---
A permission ask waited on a client's answer and watched nothing else, so `session/cancel` and `_yi/child_abort` left the turn, or the child, blocked until something answered. Every session now hands its broker a stop check that reads its abort signal. The ACP asker polls it every 100 ms, drops the pending request and rejects, and the journal records the reject as `stopped`, not as a person's deny.
