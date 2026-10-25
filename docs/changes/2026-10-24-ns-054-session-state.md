---
issue: Closes #1490, Refs #1048, Refs #1442
raise: tests +135, comments +1, crate runtime +3
---
A forked child is seeded the way compaction would cut the parent. `fork=N` used to count every user-role message as a turn, host notices included, so a parent whose last messages were lifecycle notices handed a `fork=2` child two notices and none of the request. It now counts only messages whose attribution reads as typed. The seed is walked newest first with compaction's bytes/3 estimator against the child's window less the compaction reserve, and it opens at the first cut point that fits (`compaction::fitting_start`), so a budget that cut right after a tool call no longer opens the seed on that call's orphaned result. The seed is appended to the child's session file before it is set in memory, as a partition is, so a service that crashes and is respawned from its kept transcript still opens on it. `seed_for_fork` moved from `subagent.rs` to `subagent/reader.rs`, beside `reader::seed`, which now takes the fork seed and the partition together.

Docs: YI_DESIGN section 11 (Child) says what `fork=N` counts and that the seed is kept; the ARCHITECTURE feature-ledger row for kernel, ipython and `rlm()` subagents names the respawn journey test.

Members: HB-fork-seed.
