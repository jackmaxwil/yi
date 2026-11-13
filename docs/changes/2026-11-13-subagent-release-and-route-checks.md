---
issue: Refs #2168, #2169, #2170
raise: crate runtime +157, tests +117, comments +6
cut: none crates/runtime/src/lane/handback.rs
cut: none crates/runtime/src/subagent/build.rs
cut: none crates/runtime/tests/recursion_e2e.rs
growth: +157 the fork-time snapshot with the force-discard guard, the cap census and its refusal texts, the loopback probe; weighed for deletion: the route registry and find_models marking, the kernel denial note and the ipython skill example were all cut from this change after review, and `reserved` and `release_build` moved to build.rs so subagent.rs does not grow
---
A fan-out whose children died on their first model call could leave the parent unable to spawn or clean up. A child forked with `rlm.tree()` carries the parent's uncommitted files, and neither `merge_worktree` nor `discard_worktree` would release it, so `delete_subagent` refused forever. Folding such a finished child with `rlm.tree().merge(h.tree)` now lets it be deleted (a plan-managed child is left alone). `rlm.discard_worktree(name)` works on it while it holds only the files it was forked with, and `force=True` drops changes the child made on top. The refusal names calls that work.

A child that failed before its first tool call no longer counts toward the eight-child cap. A refused spawn says how many children are held and in what state (`8 of 8 children held (2 failed, 5 finished, 1 running)`), and a duplicate name names the child that holds it and the call that frees it.

`rlm.run` now refuses a model whose `base_url` is a loopback port that refuses connections, before taking a slot or a worktree.
