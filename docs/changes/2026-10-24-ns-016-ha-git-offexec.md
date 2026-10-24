---
issue: Closes #1457, Refs #1442
raise: tests +86, crate runtime +13
---
A worktree hand-back no longer holds the session's runtime while git works. `rlm.merge_worktree`, `rlm.discard_worktree` and `rlm.delete_subagent` ran their twenty to thirty git processes inside the async handler on the session's one-thread runtime, as did the lane settle of a lease expiry, so a slow repository hook stalled every other host call, `rlm.status` included, for the whole hand-back. `rlm.run` and `rlm.result` already moved their blocking work to the blocking pool; these now share one helper with them. A post-merge hook that sleeps two seconds used to hold a status answer for about that long, and it now answers within 200 ms.
