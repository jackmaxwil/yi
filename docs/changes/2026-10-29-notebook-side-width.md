---
issue: Closes #1734
raise: crate console +47, tests +41
---
The console's notebook pane opens beside its chat at two fifths of the pane area instead of half, so a freshly opened notebook is a fifth narrower at the same window size. The share is one named constant in `crates/console/src/app/diffs.rs`; a width the user drags is theirs and is never re-applied, and the Review side keeps the half every other split takes.
