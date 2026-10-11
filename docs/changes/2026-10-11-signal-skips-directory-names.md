---
issue: Closes #1230
raise: tests +22, comments +1, crate tools +4
---
The bash output reducer keeps lines naming an error or a failure from the middle of a long output, and it judged that by substring over the whole line, so a directory name counted: under a HOME at `~/Development/.yi-homes/keep-failed`, every line naming a path there became a signal. An EPERM line in the middle of a contained run's output was kept for its path alone, and `sandbox_seam::a_refusal_deep_in_long_output_is_remembered` failed on macOS in any worktree whose name held `fail`, `error` or `assert`, at any commit. In a word that is an absolute or home-relative path (one starting with `/` or `~`), a signal word now counts only in its last component, so `/…/keep-failed/x.log: Operation not permitted` no longer counts while `/…/src/fail_fast/run.rs:12: error: …` and `failed/total: 12/300` still do. A path ending in such a directory, or a relative path through one, still counts.
