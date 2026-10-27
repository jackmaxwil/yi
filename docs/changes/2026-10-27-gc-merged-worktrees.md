---
issue: Refs #1675
raise: crate cli +10, crate runtime +113, tests +103, comments +5
---
`yi gc` now also judges the repository's linked worktrees. A worktree on a branch already merged into `origin/main` or `main`, with a clean tree, no session lock and no current directory inside it, has its `target/` deleted and is removed with `git worktree remove`; without `--fix` the line reads `would remove`. Every kept worktree prints why: detached HEAD, locked, unmerged branch, a branch still at the base tip, or the uncommitted path count.
