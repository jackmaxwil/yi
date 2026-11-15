---
issue: #2225
growth: +392 the host sampler (load, memory, disk), the pure admit decision, shared bounded probe runner, capped lane refusal retry (disk refusals only) and their tests; weighed for deletion: the fixed 8-child, 16-family and plan width ceilings are now defaults of 0 (no ceiling), so nothing was added beside them
decision: Admit children and lanes by host headroom, never by a fixed count | a count of 8 refused work a roomy host could run and let a loaded one thrash | reversible via family.max_children, family.cap and plan.width_max levers set above 0
cut: none crates/runtime/src/subagent.rs
cut: none crates/runtime/tests/levers.rs
cut: none evals/tests/test_levers.py
shares: #2234 adds one `pub mod admission;` line to crates/runtime/src/lib.rs, away from its lines
shares: #2205 adds one `pub mod admission;` line to crates/runtime/src/lib.rs, away from its lines
raise: crate cli +1, crate runtime +391, tests +244, comments +2
---
A parent no longer stops at eight children, and a family no longer stops at sixteen live sessions. A new child or lane is admitted while the host has room: the one-minute load average per core, free memory (growing with each live child) and free disk on the lanes volume. The first child is always admitted on load, so work can start on a busy host. A refusal names the resource, what the host has and what it needs, and says to wait or reap a child with `rlm.delete_subagent`.

`family.max_children`, `family.cap` and `plan.width_max` now default to 0, meaning no ceiling, and still accept a value up to their old tops (4096) for a run that wants one.

A lane claim the host refuses on disk first runs the lane gc, which removes every lane proven merged and clean, then judges again and refuses with the same named numbers if the host is still short. A refusal on load or memory returns at once, since the gc frees only disk. Performance: a disk refusal costs one whole-pool sweep (one `git worktree list`, a merged-and-clean judgement and a lock per lane tree) plus a second host sample, paid on every disk-refused claim; a load or memory refusal costs one host sample and no sweep.
