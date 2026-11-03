---
version: 0.889.0
issue: Closes #1941
raise: tests +112, crate runtime +40
---

The round-four answer to the bot on the Tree primitive. `stage::folds` is now the one spelling of the git-2.43 merge-tree gate, read by the fold itself, by the hand-back's merge-tree test and by the tree verbs, so a fourth copy cannot drift from the others, and `stage::ATTR_SOURCE_SINCE` is the one floor both roads read, since the fold and the hand-back both pass `--attr-source`; below 2.43 the tree-verb tests assert the refusal that names the floor, and a test that shims the host git to 2.34.1 runs that arm on every machine. `settle::status_row` is the one reader of a `git status --porcelain -z` record, which the fork's parent-state walk and the settle path list both call, so the rename rule has one owner; the merge into main took main's name for it and kept this branch's rename skip. `tree.discard` has its half that matters: a child's tree is refused by name and still stands after the refusal. The classifier's protected branch is read through `protected_branches`, whose test runs a `master` trunk and then a `main` one, so hardcoding `main` fails.
