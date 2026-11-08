---
version: 0.983.0
issue: Closes #1935
raise: tests +125, comments +3, crate runtime +101, crate tools +14, crate permission +9
growth: +125 is the review rounds' work: a registry refused to an em dash renderer already holding it, the outbox's no-link descent with two tests, one encoder for the git config a contained spawn gets, `refresh` reading segments instead of text, and the sweep's typed outcome. Weighed for deletion: the second encoder (`open` now calls `config_env`), the submodule status argv (now [`STATUS`]), the substring scan in `refresh`, and the outbox tests' local scratch helper (now the shared `Scratch`).
---
Moves with no behaviour change, so the files past their line cap have room for the tree work: `BranchName`, `Sha` and `Head` to `lane/names.rs`, a kernel child's hand-back to `lane/handback.rs`, the spawn-kwarg types to `subagent/build.rs`, `job_profile` to `kernel_state.rs`, and check capture to `goal/check.rs`.

A container sweep that exits nonzero now reports its exit code instead of a sweep timeout; only a sweep that hits `SWEEP_BOUND` reports the bound. The outbox tests use the shared scratch cleanup.
