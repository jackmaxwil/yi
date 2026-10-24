---
issue: Closes #1467, Refs #1442
raise: tests +133, comments +2, crate runtime +36
---
`rlm.merge_worktree` now says which files a child's work changed. The reply gains `files`, a list of `{path, added, removed}` read from `git diff --numstat` between the lane's base and the candidate (or the candidate's merge base with the parent's `HEAD` when the lane has lost its base). Every reply carries it, a `merged: false` conflict included, so a parent deciding what to do about a conflict sees what the child touched without a second call. The paths come from `-z` output, so a name with a space or an accented letter arrives as written rather than quoted. The list is read uncapped, because the 30 KB git capture the rest of the hand-back uses would cut a long list in the middle and splice an omitted-bytes row into it; a binary file counts 0 added and 0 removed.
