---
issue: Closes #1941
raise: tests +55, crate runtime +45
---

The round-four answer to the bot on the Tree primitive. `stage::folds` is now the one spelling of the git-2.43 merge-tree gate, read by the fold itself, by the hand-back's merge-tree test and by the tree verbs, so a fourth copy cannot drift from the others; below 2.43 the tree-verb tests say in a row that they skipped rather than returning green. `settle::status_records` is the one reader of `git status --porcelain -z`, which the fork's parent-state walk and the settle path list now share, so the rename rule has one owner. `tree.discard` has its half that matters: a child's tree is refused by name and still stands after the refusal. The classifier's protected branch is read through `protected_branches`, whose test runs a `master` trunk and then a `main` one, so hardcoding `main` fails.
