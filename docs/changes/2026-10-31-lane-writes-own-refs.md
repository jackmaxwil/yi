---
version: 0.866.0
decisions: D526
issue: Closes #1930
decision: a linked worktree's sandbox writes its admin dir, the shared objects, its own branch and the stash, never the rest of the common dir | a contained process deleted a sibling's ref and moved trunk | restore the common dir in lane_writes
raise: tests +722, comments +71, crate tools +163, crate runtime +427, crate permission +18, crate types +5
growth: +210 a wipe that says what it left (a submodule's worktree survives `checkout -f` and `clean -qfd`), the one submodule count the claim and the listing now both read, and a guarded directory refused for a grant whatever the host carries; weighed: a second listing reader, cut since one read answers both, and a cache of the seal, left to the perf row
---
A lane can no longer touch trunk, a sibling's branch or `packed-refs`. Its sandbox writes its admin dir, the shared objects, its own branch and the stash. The branch comes from the host's record of the slot, and a tree whose record does not parse gets none, since `HEAD` itself is contained-writable.

A new local branch, a tag, `git worktree add`, LFS and rerere are refused in a lane. The refusal now asks on the next call, and an approval runs the command outside; before, `git switch -c x` repeated the same refusal forever and nothing asked. A contained git no longer runs auto maintenance, which printed a harmless `packed-refs.lock` error on commit.

A grant of `$HOME`, `~/.yi` or a credential store is refused whether or not the host builds a Seatbelt profile, so `permissions.allowWrite` on one exits 2 on every host instead of on a Mac alone. A wipe that could not read the tree after it reports that failure, and a survivor is named for what held it: a submodule's worktree or a directory `clean -fd` left in place.
