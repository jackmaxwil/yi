---
issue: Closes #1940
growth: +197 is the flight states, the bounded capture and the record. Weighed for deletion: a capture per call on the tool's own thread, which is 42-96 ms a call in a tool's wait; and no guard, which leaves a tree too large for it running git after every call
raise: tests +286, comments +16, crate tools +53, crate types +16, crate runtime +146
decision: A mutating tool call is a capture point, off the tool's thread, and restore scope stays the turn's pair of checkpoints | a tree's history is what bisecting and a per-child log read | drop note_mutating_call
---
A tool call that writes or runs now leaves a `custom{tree_snapshot}` entry naming the tree it left, captured on a background thread so the tool never waits. Calls during a capture coalesce into one more. `/undo` reads none of these and still goes back to the turn's start.

A capture past 500 ms, or killed at 5 s, turns per-call snapshots off for that tree, since background git must not load a checkout too large for it. Each killed capture clears its own lock first.
