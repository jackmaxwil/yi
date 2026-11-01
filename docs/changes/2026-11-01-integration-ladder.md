---
issue: Closes #1942
decision: a branch whose content is in trunk counts as merged however it landed | squash-merged branches kept lanes and disk forever | restore merge-base --is-ancestor in Pool::judge and Pool::detach
raise: tests +288, comments +6, crate runtime +191
growth: +191 the content rung, its loud path cap and three fixtures for the multi-commit squash this issue is about; deletion weighed: the second trunk fixture went, EmptyDiff stays the one rung the others cannot answer
---
`yi gc` and a lane's release now recognise a squash-merged branch. They ask, cheapest first: the same commit, an ancestor, an empty three-dot diff, the same tree, the branch's changed paths agreeing with trunk's, and matching patch-ids. A report line says which: `removed <path> (merged: same content)`. Dirty and locked trees stay as before, and a question git could not answer keeps the branch: a release now names that reason instead of calling it unmerged, and an orphaned slot asks the same question. Past 256 commits on either side of the fork the patch-id walk stops and says so, and so does a branch whose changed-path list is past 1024. A dry-run `yi gc` writes nothing: no rung merges or stores an object to answer.
