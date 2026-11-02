---
issue: Closes #1935
raise: tests +2, comments +3, crate runtime +25
ledger=: contained git: a mirror and an outbox | 8 | core | H | M | a contained `git fetch` reads a host-run mirror and a `git push` lands in an outbox the host forwards after the spawn, with no question unless it is trunk, `main`, `master` or a tag; force and delete still ask (`crates/runtime/src/lane/remote.rs`)
---
Moves with no behaviour change, so the files past their line cap have room for the tree work: `BranchName`, `Sha` and `Head` to `lane/names.rs`, a kernel child's hand-back to `lane/handback.rs`, the spawn-kwarg types to `subagent/build.rs`, and `job_profile` to `kernel_state.rs`.
