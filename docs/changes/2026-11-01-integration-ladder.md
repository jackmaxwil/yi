---
issue: Closes #1942
decision: a branch whose content is in trunk counts as merged however it landed | squash-merged branches kept lanes and disk forever | restore merge-base --is-ancestor in Pool::judge and Pool::detach
raise: tests +403, comments +11, crate runtime +239
growth: +239 the content rung, the quoting and cap fixes the review found in it, and three fixtures for the multi-commit squash this issue is about; deletion weighed: the second trunk fixture went, EmptyDiff stays the one rung the others cannot answer
---
`yi gc` and a lane's release now recognise a squash-merged branch. They ask, cheapest first: the
same commit, an ancestor, an empty three-dot diff, the same tree, the branch's changed paths
agreeing with trunk's, and matching patch-ids. The path list is read NUL-separated and every path
goes back as a literal pathspec, so a name git would quote is compared rather than read as no
change at all. A report line says which rung: `removed <path> (merged: same content)`. Dirty and
locked trees stay as before, and a question git could not answer keeps the branch: a release names
that reason instead of calling it unmerged, and an orphaned slot asks the same question. Past 256
commits on either side of the fork the patch-id walk stops and says so; a changed-path list past
1024 skips that rung and lets the walk answer, and speaks only when the walk cannot answer either.
A dry-run `yi gc` writes nothing: no rung merges or stores an object to answer. A path that is
not UTF-8, or a merge commit with content of its own, keeps the branch.
