---
issue: Closes #1937
growth: +972 is the fork, its copy, the two carries with their limits, and first use. Deleted beside it: the worker permit, Purpose::Worker and claim_child_lane. Weighed for deletion: .worktreeinclude and .yi/setup, kept since each saves every child rediscovering the same step; a name list for cargo units, which carries an in-tree patched crate and tests the parent
raise: tests +860, comments +63, over-cap +1, crate runtime +882, crate tools +74
decision: A worktree child is a linked worktree under ~/.yi/forks, cut at HEAD, carrying only cargo units whose dep-info names nothing in its tree; no slot or worker permit | 64 GB and 524 s at 128 children | restore claim_child_lane. Amends D119, D286
---
A worktree child no longer claims a lane slot. It takes a linked worktree of its own under `~/.yi/forks`, cut at the parent's `HEAD`: a directory clone on APFS, a plain checkout elsewhere or when the parent has submodules. Sessions keep their lanes. `lanes.slots` no longer bounds children; family caps do, and `family.cap` tops out at 129, since the root's kernel counts as a live session and 128 means 127 children otherwise.

A child's first writing command carries the third-party cargo units from the parent's `target/debug`, those whose dep-info names nothing in its tree, then syncs the toolchain and runs a committed `.yi/setup`. A package with one unit that names the tree loses all of them, because a clone at a new path otherwise reads Fresh and tests its parent. `.worktreeinclude` and `carry=[...]` on `rlm.run` copy files that are both listed and ignored, and `.venv` never carries. Discarding a child stops the jobs still running in its tree.
