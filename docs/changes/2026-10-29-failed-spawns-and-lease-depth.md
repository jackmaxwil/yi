---
issue: Closes #1571
raise: comments +10
raise: crate cli +9
raise: crate runtime +100
raise: tests +182
---
A spawn refused after its directory was made leaves nothing behind. The directory is created before the container placement, the lane claim and the build, and each of those can refuse; a guard removes it unless the child's record is listed, so the family names only children that ran, and a refused second worker in a one-lane pool leaves no `sub-*` in the parent's `children/`.

`/land` names the cause git gave it. A merge git refuses for a file the lane has uncommitted changes to starts no merge at all, and `merge --abort` then answered `fatal: There is no merge to abort (MERGE_HEAD missing)` in place of git's own message; with no `MERGE_HEAD` the landing now reports git's refusal, naming the lane to resolve it in. A conflict list stays fenced and bounded at twenty files, and past that it carries its own row: `[20 of 21 conflicted files (CONFLICT_FILES_MAX 20); git diff --name-only --diff-filter=U in <lane>]`. The landing-failed steer is bounded at 2,000 characters with the same kind of row.

A lease expires at every depth. The clock and the loop that runs expiry lived in the plan timer, which only a depth-0 session wired, so a grandchild below depth 0 whose lease was revoked had its grace served by nobody and its lane stayed orphaned. The loop is now the host's, in `lease.rs`, spawned by `attach_runtime` at every depth and woken by the due time a revocation registers; the plan timer keeps only its stuck and dispatch jobs. `yi ask`, `yi rpc` and a solo TUI end by closing their host, so the children still running are revoked under their own `parent_close` and their lanes come back, rather than outliving the session that owns them.
