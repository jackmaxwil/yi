---
issue: Closes #1942
decision: a branch whose content is in trunk counts as merged however it landed | squash-merged branches kept lanes and disk forever | restore merge-base --is-ancestor in Pool::judge and Pool::detach
raise: tests +135, comments +6, crate runtime +83
---
`yi gc` and a lane's release now recognise a squash-merged branch. They ask, cheapest first: the same commit, an ancestor, an empty three-dot diff, the same tree, a merge that adds nothing, and matching patch-ids. A report line says which: `removed <path> (merged: patch-id)`. Dirty and locked trees stay as before, and a question git could not answer keeps the branch. Past 256 commits the patch-id walk stops and says so.
