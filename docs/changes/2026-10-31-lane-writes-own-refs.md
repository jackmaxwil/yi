---
issue: Closes #1930
decision: a linked worktree's sandbox writes its admin dir, the shared objects, its own branch and the stash, never the rest of the common dir | a contained process deleted a sibling's ref and moved trunk | restore the common dir in lane_writes
raise: tests +122, comments +8, crate permission +18, crate tools +82, crate types +5
---
A lane can no longer touch trunk, a sibling's branch or `packed-refs`. Its sandbox writes its admin dir, the shared objects, its own branch and the stash. The branch comes from the host's record of the slot, and from `HEAD` only when that names no trunk, sibling or other lane, since `HEAD` itself is contained-writable.

A new local branch, a tag, `git worktree add`, LFS and rerere are refused in a lane. The refusal now asks on the next call, and an approval runs the command outside; before, `git switch -c x` repeated the same refusal forever and nothing asked. A contained git no longer runs auto maintenance, which printed a harmless `packed-refs.lock` error on commit.
