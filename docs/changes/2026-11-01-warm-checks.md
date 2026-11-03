---
version: 0.886.0
decisions: D536
issue: Closes #1939
growth: +412 is the pool, scrub, snapshot and ticket table with their limits. Weighed for deletion: sharing nothing, a target per check, which is the 176 s at 128 children this replaces; and the ticket table, which a cell's 600 s abort makes necessary
raise: tests +416, comments +24, crate runtime +317
decision: Kernel checks and the plan's candidate check run in a per-repo pool of node.slots warm directories, scrubbed to cargo's bookkeeping, and report exit and output only | 43 s against 176 s at 128 children | call run_check_in on the path again. Amends D475
---
A child's `check=` now runs in a warm directory rather than in the child's own tree. `merge_worktree` settles first and checks the settled commit, so a job still running refuses it at once, and a check's unignored writes no longer land with the child's work. `rlm.result` on a fork checks a snapshot of the tree, and a plan's verifier claims from the same pool.

A check past its budget or its queue reads as an abstention, never as a red, and the reply names the bound. A check that writes or edits frees its directory all the same.

A warm directory keeps only cargo's bookkeeping. `.venv` and `node_modules` go between checks: a check could otherwise pass on what the last borrower installed, and only cargo re-verifies what it keeps. A repository that is not cargo pays a full install per check.
