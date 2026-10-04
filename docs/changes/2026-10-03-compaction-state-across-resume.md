---
issue: Closes #958
raise: tests +45, comments +1, crate runtime +29
---
A resumed session no longer compacts a second time on its first prompt (Closes #958). The marker that stops a compaction's kept tail from counting its pre-compaction usage, and the window chain, lived in memory only. `attach_store` now rebuilds both from the latest compaction entry, which already stores the retained tail and the window in its details, so no new field is written.
