---
issue: Closes #1707
ledger+: daemon (ACP-router supervisor + worker/root) | a root's worker is swapped for a freshly built binary through `yi worker replace --binary` (`_yi/prepare_replace`, then `_yi/restore` and `_yi/worker_replaced` for the clients), which refuses by name rather than migrating a live turn, job, child, ask or unsavable kernel, and keeps every attached client through the swap (`serve_daemon::replacing_a_worker_keeps_the_session_and_the_daemon_serving_it`, T2)
raise: crate acp +586
raise: crate cli +117
raise: crate console +15
raise: crate kernel +5
raise: crate runtime +12
raise: tests +189
raise: comments +43
growth: +712 `yi acp` loses its permission bridge to `permission.rs` (−108 lines in `lib.rs`) to pay for the swap module, so the worker's own file stays under its ceiling and nothing was duplicated to do it
---
`yi worker replace --binary <path>` swaps one root's running `yi acp` worker for a freshly built binary while the supervisor, its clients and every session's durable state stay where they are: the console keeps its pane, and the same session resumes from the ledger with its plan, mail, claims and kernel variables intact.

The supervisor keeps a generation per root, a map of the binary each root was last replaced with, and one swap in flight. The replacement asks the old worker for an inventory first (`_yi/prepare_replace`): a turn in flight, a queued prompt, an unread follow-up, a permission ask, a running child, a running job, a kernel cell, or a kernel whose namespace its snapshot cannot hold whole refuses the swap by name, and nothing is replaced. An idle worker flushes every kernel through its checked dispose, asks its inventory again, and answers `prepared`; only the supervisor's acceptance retires it, by closing its stdin, so a late answer to a refused swap leaves it serving; the supervisor spawns the requested binary in its place, restores the sessions the root owned (`_yi/restore`, no replay), tells every attached client to resume (`_yi/worker_replaced`), and only then answers the verb. A worker that does not answer within 30 s is refused; one that does not close is killed at the same deadline rather than left serving a build the caller already replaced; a root whose sessions do not restore within 60 s is reported failed rather than successful.

The generation is what keeps the old build from coming back: a close from a superseded generation is ignored, and the binary a root was last replaced with outranks `current_exe` when that root's worker is later spawned again. Every client request bound for a root between two builds is refused with the replacement's id and the generation pair instead of being dropped.

`just dev` rebuilds and then calls the verb, so a dev loop no longer kills `yi serve`; a daemon too old to know the verb says so in its own refusal.

Async methods (`_yi/prepare_replace`) are answered from the worker's request loop, and the final kernel flush is awaited before the answer, so a variable cannot be lost to a replacement that already reported success.

Deliberate ceilings of this first version: a replacement never migrates a live turn, a job or a pending ask (they are refused, never carried); a session whose mailbox or scheduler was bound in the old worker is re-bound from the ledger by `_yi/restore` but its next wake is not proven against a missing durable subscription; the kernel's event log post-mortem keeps only the last 200 events per variable, unchanged by this landing.
