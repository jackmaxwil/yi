---
issue: Closes #1390
growth: +215 weighed the Kept LRU cap and trim, the open_shared constructor, and the child-4 weak assertion against deletion
raise: crate runtime +207, crate session +8, comments +12
---
A long-running parent no longer keeps every reaped child's transcript in memory, and the `history.search` corpus is held once per process (Closes #1390). `SubagentHost::reaped` pinned the full session store of every child it reaped; it now holds the four most recently used file-backed children open and reopens the rest from their session file on demand, keeping a weak reference so that an inbox write and a `history://` read never append through two live stores of one file. In-memory children, which have no file, stay held. The search corpus was built per kernel, so each live kernel carried its own copy of the repository's session text and every query cloned all of it; one process-wide corpus now hands each file's units out by `Arc`. Its freshness key adds the file's inode to its length, so a session replaced by rename at the same length is re-read rather than served stale.
