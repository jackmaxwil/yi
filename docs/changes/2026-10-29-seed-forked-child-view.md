---
issue: Closes #1767
raise: crate runtime +23, tests +130, comments +3
---
A forked child was seeded from the parent's stored history, so it re-read every tool result its
parent's own requests had already replaced with a `[reclaimed: …]` placeholder: the bytes, not
the view. `SubagentHost` now reads the parent through `AgentSession::view_handle`, which renders
the parent's cuts the way its own requests send them, so a `fork=all` child starts from what its
parent's model still sees. The seed's budget was the window less compaction's reserve, 92% of a
128,000-token window; it is now 80% of the child's own route window, so the child's first request
still has room for its brief and the turn it was forked to take, and the `[… fork kept K of N`
row names that ceiling, the window it came from and `history://parent/since/0`.
