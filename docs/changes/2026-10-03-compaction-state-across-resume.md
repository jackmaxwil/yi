---
issue: Closes #958
raise: tests +133, comments +2, crate runtime +38
---
A resumed session no longer compacts a second time on its first prompt (Closes #958). The marker that stops a compaction's kept tail from counting its pre-compaction usage, and the window chain, lived in memory only. `attach_store` now rebuilds both from the latest compaction entry, which already stores the retained tail and the window in its details, so no new field is written. A branch left with no compaction entry (a rewind past one) goes back to the initial window and clears the marker.
