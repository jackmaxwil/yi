---
name: lane-pool-reclaim
description: A "no free lane" error means orphaned worktrees hold the pool; `yi lanes` lists them and `yi lanes reclaim` frees one.
type: project
---
The pool hands a new session a worktree; a killed session leaves its lane orphaned.

**Why:** NewSession(PaneId) fails with "lane: no free lane: 1 held, 2 orphaned of 3" after a crashed run.

**How to apply:** run `yi lanes`, then `yi lanes reclaim` before retrying `/new`. Never delete a lane directory by hand: the pool's ledger still counts it.
