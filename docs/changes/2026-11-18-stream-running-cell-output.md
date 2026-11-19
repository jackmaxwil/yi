---
issue: Closes #2286
growth: +228 the live-output buffer with its tick and loud cap and its two unit tests (cell_live.rs, the bulk of runtime +211) and the ACP projection (acp +17); weighed for deletion: nothing emitted ToolExecutionUpdate at all, so the producer is new, and reusing the kernel's existing uncapped on_stream hook and the session event stream avoided any change to the loop or the tool trait
raise: crate runtime +211, crate acp +17, tests +146, comments +6
---
A client now sees a running `ipython` cell's output while it runs. The runtime sends a `tool_execution_update` event at most ten times a second per stream, and the standard ACP projection carries it as `_yi/tool_progress` with `toolCallId`, `stream` and `text`, a chunk of new output and never the whole. A flood past 8192 characters in one tick keeps the first 8192 and says how many were seen; the tool result still holds the whole output and stays authoritative.
