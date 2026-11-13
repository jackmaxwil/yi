---
issue: Refs #2168, #2169, #2170, #2171
raise: crate runtime +207, tests +195, comments +7
cut: none crates/runtime/src/lane/handback.rs
cut: none crates/runtime/src/subagent/build.rs
cut: none crates/runtime/tests/recursion_e2e.rs
growth: +203 the carried-tree snapshot and force-discard guard (fork.rs, handback.rs), the cap census with its refusal text and the loopback route probe with the failing-route note (build.rs, models.rs, runs.rs); weighed for deletion: moved `reserved` and `release_build` out of subagent.rs so that file does not grow, and left quota-class refusal out because it needs a paid probe
---
A fan-out whose children died on their first model call could leave the parent unable to spawn or clean up. A child forked with `rlm.tree()` carries the parent's uncommitted files, and neither `merge_worktree` nor `discard_worktree` would release it, so `delete_subagent` refused forever. Folding such a finished child with `rlm.tree().merge(h.tree)` now lets it be deleted. `rlm.discard_worktree(name)` works on it while it holds only the files it was forked with, and `force=True` drops changes the child made on top. The refusal names calls that work.

A child that failed before its first tool call no longer counts toward the eight-child cap. A refused spawn says how many children are held and in what state (`8 of 8 children held (2 failed, 5 finished, 1 running)`), and a duplicate name names the child that holds it and the call that frees it.

`rlm.run` now refuses a model whose `base_url` is a loopback port that refuses connections, before taking a slot or a worktree. `rlm.find_models` marks a route whose last child died on its first model call. A route that fails for quota or the network cannot be told without a paid call, so it is marked, not refused.

The sandbox denial note for kernel cells says to fetch with the bash tool and load the file in the cell, and the ipython skill carries the forge example.
