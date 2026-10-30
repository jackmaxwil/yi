---
version: 0.807.0
issue: Refs #1675
growth: +217 live-HEAD revalidation and slot cleanup; cached HEAD/branch state, duplicate status walks and slot-clearing copies were removed
raise: crate cli +10, crate runtime +207, tests +394, comments +5
---
`yi gc` now also judges the repository's linked worktrees. A worktree whose branch or detached HEAD is already merged into the resolved main base, with a clean tree, no live session or outside-worktree lock and no current directory inside it, has its `target/` deleted and is removed with `git worktree remove`; without `--fix` the line reads `would remove`. A stale pool lock is unlocked only after its session flock is free. Every kept worktree prints why: locked, unmerged commit, a branch still at the base tip, or the uncommitted path count.
