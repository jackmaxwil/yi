---
issue: Refs #2168, #2169, #2170
cut: none crates/runtime/src/lane/handback.rs
cut: none crates/runtime/src/subagent/build.rs
cut: none crates/runtime/tests/recursion_e2e.rs
growth: +193 the fork-time snapshot with the force-discard guard, the cap census and its refusal texts, the loopback probe, and the fold's own discard path (a folded child settles at fold time: a job under its tree gets up to 250 ms to record its exit, not the 5 s of `rlm.discard_worktree`, and a tree whose jobs outlive that is taken, polled and handed back with a note); weighed for deletion: the route registry and find_models marking, the kernel denial note and the ipython skill example were all cut from this change after review, and `reserved` and `release_build` moved to build.rs so subagent.rs does not grow
raise: crate runtime +193, tests +236, comments +10
---
A fan-out whose children died on their first model call could leave the parent unable to spawn or clean up. A child forked with `rlm.tree()` carries the parent's uncommitted files, and neither `merge_worktree` nor `discard_worktree` would release it, so `delete_subagent` refused forever. Folding such a finished child with `rlm.tree().merge(h.tree)` without conflict now releases its worktree the way `rlm.discard_worktree` does, so it can be deleted. A tree the discard refuses, such as a plan-managed child's, is named in a `notes` row of the reply. `rlm.discard_worktree(name)` works on it while it holds only the files it was forked with, and `force=True` drops changes the child made on top. The refusal names calls that work.

A child that failed before its first tool call no longer counts toward the eight-child cap. A refused spawn says how many children are held and in what state (`8 of 8 children held (2 failed, 5 finished, 1 running)`), and a duplicate name names the child that holds it and the call that frees it.

`rlm.run` now refuses a model whose `http` or `https` `base_url` is a loopback port that refuses connections, before taking a slot or a worktree.

Folding a finished child whose tree still has a live job gives that job up to 250 ms to record its exit, where `rlm.discard_worktree` waits up to 5 s. A tree whose jobs outlive that is left in place, and the merge reply names `rlm.discard_worktree(name, force=True)`.

The memory-label eval test that feeds `evals/memory_inject.py` a judge reply with no `usage.cost` now captures the refusal and asserts its exact text, so a reply that cannot be counted against `--cap-usd` is still refused by name. The refusal's `FAIL label:` line no longer reaches a gate log, where the commit hook read it as a failing gate and refused this branch's commits.
