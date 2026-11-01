---
issue: Closes #1935
raise: tests +2, comments +2, crate runtime +20
---
Moves with no behaviour change, so the files past their line cap have room for the tree work: `BranchName`, `Sha` and `Head` to `lane/names.rs`, a kernel child's hand-back to `lane/handback.rs`, the spawn-kwarg types to `subagent/build.rs`, and `job_profile` to `kernel_state.rs`.
