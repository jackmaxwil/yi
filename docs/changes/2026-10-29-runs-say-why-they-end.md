---
issue: Closes #1516
growth: +253 the reason on every end, its five loop exits and the done.rs seam the file cap forced; weighed for deletion: the duplicate end() body, yi ask's second inference of the end, and the blocked-row skip that no reachable list needed
raise: crate cli +59, crate loop +15, crate runtime +139, crate tui +2, crate types +38, tests +607, comments +8
ledger~: goals + autonomous | p, error-turn blocks | p released after three runs that changed nothing
---

A run now says why it ended. `AgentEnd` carries `reason`, a closed `RunEnd` vocabulary — `stop`, `error`, `aborted`, `length`, `guard`, `drained`, or an unknown tag re-emitted verbatim — filled at all five of the loop's exits, by the debug failed-turn end as `error`, and left absent by synthetic ends; a producer that sends no reason leaves the bytes of `agent_end` exactly as they were, so a Pi client and an older driver read the same line. `yi ask` decides its exit and its `no_answer` line from that reason instead of rebuilding it from the last reply's stop: a run whose `agent_end` says `error` closes as `error` and exits 1 even when the reply before it stopped cleanly.

The stop-claim re-drives stop firing on text that only looks like a claim. `claims_impossible` matches each phrase at word boundaries, so "no validation errors" no longer reads as "no valid", and an answer-file re-drive weighs only a data path the prompt itself named: a `package.json` version or a yaml port written during the turn no longer earns one, while the answer file the prompt asked for still does.

A `done` with no target keeps blocked rows open. The call reached every row only when nothing was runnable — a list whose rows all wait on someone else — and closed them with their asks; it now closes pending rows only and names each blocked row it kept in the result note.

A goal releases its continuation after three runs that changed nothing. `GoalService` counts continuation runs that ended on an error or called no tool, and at three it stops continuing, marks the goal blocked and steers one notice naming the cap and the two ways back (`goal.update` to finish, `goal.objective` to restate). A single failed turn no longer blocks a goal on its own, and a run that called a tool resets the count.
