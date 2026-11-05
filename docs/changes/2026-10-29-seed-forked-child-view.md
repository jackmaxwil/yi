---
version: 0.933.0
issue: Closes #1767
raise: crate runtime +56, tests +155, comments +4
ledger~: kernel + ipython + rlm() subagents | written to its transcript | a `fork=all` child from the parent's view with its cut results as the placeholders its own requests carry, both inside 80% of the child's route window behind a `[… fork kept K of N` row when that ceiling cut, written to the child's transcript
---
A forked child was seeded from the parent's stored history, so it re-read every tool result its
parent's own requests had already replaced with a `[reclaimed: …]` placeholder: the bytes, not
the view. `SubagentHost` now reads the parent through `AgentSession::history_handle`, which renders
the parent's cuts the way its own requests send them, so a `fork=all` child starts from what its
parent's model still sees; each placeholder names the parent and points at
`history://parent/<entry>`, since `self` in the child is the child's own store. The seed's budget was the window less compaction's reserve, 92% of a
128,000-token window; it is now 80% of the child's own route window, never past its compaction threshold (which binds
under an 81,920-token window), so the child's first request
still has room for its brief and the turn it was forked to take, and the `[… fork kept K of N`
row names that ceiling, the window it came from and `history://parent/since/0`.
