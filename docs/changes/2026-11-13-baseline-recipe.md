---
issue: Closes #2184
---
`just baseline '<nextest filterset>'` runs a test on the fork point in a throwaway git worktree (a cold build into a temp target dir, deleted after), then on HEAD, and prints both exits, so "does it fail without my change" no longer needs a stash. It never touches the working tree. `scripts/suite_home.sh`, which recipes source, no longer kills a shell that sources it for something that is not a recipe: its guard now runs before it sets any shell option, so it prints why on stderr and returns 1 and leaves the caller's options as they were. Before, sourcing it from the bash tool exited silently with `exit code: 1`, and once skipped a `git stash pop` that was meant to follow.
