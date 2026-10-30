---
issue: Closes #1738
growth: +773 the tree's walker and `.gitignore` matcher, weighed against a new `ignore` dependency (§18.3 absent, deny.toml unset), a daemon RPC seam for a listing, and borrowing `yi-tools`' walker (the boundaries allowlist forbids console -> tools); the smallest of the three that keeps the issue's ignore rule
raise: crate console +773
raise: comments +22
raise: tests +249
---
The console's Files button opens a tree of the workspace instead of the command palette: the popup lists the focused root folded by folder, a click or ⏎ on a folder folds it, a click or ⏎ on a file opens it in an editor pane and closes the tree, and ← → walk out and in with the cursor kept on a row the frame draws. `.git` and every path a `.gitignore` declares stay out, read from the root and from each folder the walk enters, so a nested rule judges only what is under it and a `!` line puts back what an earlier line hid; a walk past 20,000 files keeps the first 20,000 in name order and says so in the popup's last row, naming what it kept of the total, the cap and where the rest is. `⌥e` and the header button reach the same tree, `r` rereads the disk, and a root the walk cannot read says why instead of drawing an empty popup. File size cost: `app.rs` and `render.rs` are past 1,000 lines at the fork, so the tree's keys and clicks live in `files.rs` with the model, and the two popups share one dispatch and one popup arm rather than growing either file.
