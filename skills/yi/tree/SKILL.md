---
name: tree
description: Fork, check and merge uncommitted files from the kernel with rlm.tree
covers: tool:ipython
---

# Trees

A `Tree` is a set of files you can copy cheaply, committed or not. Reach for one when the work depends on edits nobody committed, when several attempts should start from the same files, or when you want a verdict on files without leaving your own tree. `isolation='worktree'` remains right for plan documents and for a child whose result should land as commits: that child starts clean at `HEAD`.

| Want | Do |
|---|---|
| A child that sees your uncommitted edits | `f = await rlm.tree().fork("fix")`, then `h = await rlm.run(brief, tree=f)` |
| Its files back, as edits, no commit | `await rlm.tree().merge(f)` |
| A verdict on a tree | `c = await f.check("cargo test -p x")`, then read `c.state` |
| To go back | `t = await f.head()` before, `await f.rewind(t)` after |
| What a child changed, with no kernel | `read("tree://fix/")` |
| Gitignored files in the fork | `carry=[".env"]` on `fork`, or list them in `.worktreeinclude` |

## How calls combine

Best of N: fork once per attempt, give each to a child, `check` each, merge the first green tree and `discard` the rest. A seen-red run: fork the fixed tree, `rewind(paths=[fix])` to the unfixed files, and `check` there, with your own work intact. Relay: `t.fork(at=id)` from a `head()` an earlier step noted.

`f.diff()` carries `would_conflict`, the paths a merge into this session would stop on. Ask before spending a long check on a tree that cannot land.

## When it goes wrong

- `merge` says `merged 1 of 2`. A tree that conflicted moved nothing. Read both sides with `diff(path=...)`, change one, merge again.
- `check` answers `state: running`. It is still going in a warm directory or waiting for one. The same call with the same command reads that run, and a cell's abort does not end it.
- `check` answers `state: abstain`. That is no verdict on the work: `why` names the budget, queue or sandbox that stopped it. Do not read it as red.
- `would_conflict` is `None`. The trial failed, so nothing is known. It does not mean clean.
- `rlm.merge_worktree` refuses a child on a forked tree. A commit would claim your uncommitted edits as the child's, so merge by tree.
- `f.bash(cmd)` has loopback only. A command that needs the network belongs in a child's own bash, or in `bash()` in your own tree.
