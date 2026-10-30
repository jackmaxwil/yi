---
issue: Closes #664
raise: crate runtime +10, tests +36, comments +2
---
Deleting `~/.yi/lanes` no longer strands the pool. Git keeps a worktree registered after its directory is gone, and a lock a dead session left on it made the next claim refuse the slot the pool owns, with every later claim after it: `fatal: '<path>' is a missing but locked worktree`. A fresh claim now clears git's registration of its own slot path before it adds the worktree back, whichever shape the stale registration has, and the rebuilt slot checks out the new session's branch with the repository holding one registration for that path.
