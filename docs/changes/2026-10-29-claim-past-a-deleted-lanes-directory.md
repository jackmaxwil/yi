---
issue: Closes #664
ledger+: lanes: worktree-first sessions, pooled slots, hand-back by move, landing as an event, contained warmer (absorbs B11) | a claim over a slot whose lanes directory was deleted drops every registration under the lanes directory whose tree is gone, locked ones included, and rebuilds the slot instead of refusing it
raise: crate runtime +29, tests +104, comments +4
---
Deleting `~/.yi/lanes` no longer strands the pool. Git keeps a worktree registered after its directory is gone, and a lock a dead session left on it made the next claim refuse the slot the pool owns, with every later claim after it: `fatal: '<path>' is a missing but locked worktree`. A fresh claim now drops every registration under the lanes directory whose tree is gone, locked ones included, before it adds the worktree back, whichever shape the stale registration has, and the rebuilt slot checks out the new session's branch with the repository holding one registration for that path.
