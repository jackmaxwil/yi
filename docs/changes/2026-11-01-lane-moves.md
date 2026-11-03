---
version: 0.876.0
issue: Closes #1935
raise: tests +2, comments +3, crate runtime +25
---
Moves with no behaviour change, so the files past their line cap have room for the tree work: `BranchName`, `Sha` and `Head` to `lane/names.rs`, a kernel child's hand-back to `lane/handback.rs`, the spawn-kwarg types to `subagent/build.rs`, and `job_profile` to `kernel_state.rs`.
