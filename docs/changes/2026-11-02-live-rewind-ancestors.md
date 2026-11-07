---
version: 0.976.0
issue: Refs #1442
growth: +162 bounded active-main ancestry suffix and ordered console append; weighed deleting the suffix and reconstructing hidden parent IDs in the client, which lacks the worker's saved entries
raise: crate acp +147, crate console +8, crate types +7
raise: tests +233
ledger+: console right-click menu and pane close mark | live committed notices retain bounded ordered active-main ancestry, including hidden model-change and tool-result entries
ledger+: ACP v2 server | `_yi/committed` sends bounded ordered active-main ancestry through each saved message and names any cut
---
A live committed message carries the newest active-main entries after the prior commit cursor, oldest first, alongside the saved message. The suffix uses the replay entry and byte limits, and a cut notice names the kept and total counts and the resume call that returns the complete branch. The console appends those ancestors before the message while retaining replay/live deduplication, so hidden ModelChange and ToolResult entries keep the visible tool call on the active path and Up/Enter rewind to that selected call.
