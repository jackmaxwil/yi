---
issue: Closes #1941
growth: +698 is eight verbs over names and the plumbing under them. Weighed for deletion: t.log and t.rewind, kept because a seen-red run needs both; `publish`, cut, since git commit and the outbox already land; a call log per tree, cut until a recipe asks
raise: tests +408, comments +34, over-cap +2, crate runtime +782
decision: Tree is the kernel's filesystem noun over named uncommitted files, and isolation stays for plan documents | one text at any depth, and a child that sees a person's uncommitted edits | remove register_tree and rlm.Tree. Amends YI_DESIGN sections 3, 11 and 14
---
`rlm.tree()` names a set of files, committed or not. `await t.fork("fix")` copies them cheaply, uncommitted files included, and `rlm.run(brief, tree=f)` gives a child that tree. `await f.check(cmd)` runs in a warm directory and answers green, red, abstain or still running. `await t.merge(f)` folds another tree's files in with no commit, and a tree that conflicted moves nothing. `await f.head()` names the files now, for `rewind` and `fork(at=)`. `await f.diff()` lists changes and the paths a merge would stop on. `read("tree://fix/")` lists them with no kernel, from any role.

`isolation='worktree'` is unchanged for plan documents and for children whose results should land as commits. `rlm.merge_worktree` refuses a child on a forked tree, since a commit would claim a person's edits as the child's.
