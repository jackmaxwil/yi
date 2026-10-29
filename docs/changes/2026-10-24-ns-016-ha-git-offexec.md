---
version: 0.726.0
issue: Closes #1457, Refs #1442
raise: tests +144, crate runtime +17
---
A worktree hand-back no longer holds the session's runtime while git works. `rlm.merge_worktree`, `rlm.discard_worktree` and `rlm.delete_subagent` ran their twenty to thirty git processes inside the async handler on the session's one-thread runtime, as did the lane settle of a lease expiry, so a slow repository hook stalled every other host call, `rlm.status` included, for the whole hand-back. `rlm.run` and `rlm.result` already moved their blocking work to the blocking pool; these now share one helper with them. A post-merge hook that sleeps two seconds used to hold a status answer for about that long, and it now answers within 200 ms. The hop would also have let two gathered `rlm.merge_worktree` calls publish against the parent checkout at once, which the one-thread runtime used to rule out, so the kernel's merge now takes the same process-wide integration lock the plan engine's acceptance holds, from the generation read through publication.
