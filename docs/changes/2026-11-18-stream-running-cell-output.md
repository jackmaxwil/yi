---
version: 0.1073.0
issue: Closes #2286
growth: +268 the live-output buffer with its tick and loud cap and its two unit tests (cell_live.rs, the bulk of runtime +211) and the ACP projection (acp +17); weighed for deletion: nothing emitted ToolExecutionUpdate at all, so the producer is new, and reusing the kernel's existing uncapped on_stream hook and the session event stream avoided any change to the loop or the tool trait
shares: #2256 it edits crates/runtime/src/lib.rs; this adds one `mod cell_live;` line to the module list
shares: #2237 it edits crates/runtime/src/lib.rs; this adds one `mod cell_live;` line to the module list
shares: #2239 it edits crates/acp/src/update.rs and acp_unit.rs; this adds one `_yi/tool_progress` helper and a test at the end of the test file
raise: crate acp +17, crate runtime +251, tests +163, comments +6
---
A client now sees a running `ipython` cell's output while it runs. The runtime sends a `tool_execution_update` event at most ten times a second per stream, and the standard ACP projection carries it as `_yi/tool_progress` with `toolCallId`, `stream` and `text`, a chunk of new output and never the whole. A flood past 8192 characters in one tick keeps the first 8192 and says how many were seen; the tool result still holds the whole output and stays authoritative.
