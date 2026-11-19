---
issue: Closes #2113, Closes #2281
cut: crates/cli/tests/acp_protocol.rs | fn a_prompt_dropped_before_insertion_is_answered_cancelled() -> TestResult { | a queued prompt is now answered at queueing with its place, so the test pins that answer and that a close still drops it unrun
shares: #2294 both add a field beside the prompt answer in update.rs and a re-export in lib.rs; the edits are disjoint lines
shares: #2256 both touch the session/prompt path in acp lib.rs and types acp.rs; this one only changes the queued branch
shares: #2239 both touch acp forward.rs, update.rs and the acp_protocol test file; separate hunks
shares: #2237 both add one re-export line to runtime lib.rs
raise: crate acp +15, crate runtime +37, crate types +3, tests +83, comments +6
decision: A `session/prompt` that arrives mid-turn is queued and answered at once with `{messageId, queuePlace}`, and starts-or-queues is one decision under the session's status lock (amends D351's answer-only-after-insertion for queued prompts) | a client could not tell a queued prompt from a lost one, and the start-then-queue pair of calls let a prompt sent between them start ahead of the queue | answer queued prompts at insertion again and restore the two-call fallback in `crates/acp/src/lib.rs`
---
A prompt sent while a turn runs is answered at once with its place in the queue (`queuePlace`, 1 is next) and runs in the order it was sent. The session decides start-or-queue under one lock, so no prompt can start between the two. A queued prompt closed before it ran gets no second answer.
