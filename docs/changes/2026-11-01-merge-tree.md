---
issue: Closes #1938
growth: +90 is the review's repairs. Weighed for deletion: dropping the old road, which leaves a runner on git 2.34 with no merge at all, since git before 2.43 crashes on --attr-source
raise: tests +509, comments +20, crate runtime +228, crate tools +7
decision: Kernel hand-back merges by merge-tree --attr-source, and git merge in a lane below 2.43 | a slot per merge, and a checkout to write | revert the one call. Amends D475's order
---
`rlm.merge_worktree` now merges in the object store, through `git merge-tree --write-tree`, so a hand-back takes no lane and writes no checkout. It reads attributes from the parent's commit, as `git merge` in a checkout of that commit would. A git older than 2.43, or a version that cannot be read, takes the old road; the plan engine stays on it, since it verifies in that directory.

A repository's `pre-merge-commit` and `commit-msg` hooks no longer fire on this path, and a repository with `commit.gpgsign` gets an unsigned integration commit, as the host's other commits already are.
