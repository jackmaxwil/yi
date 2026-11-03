---
issue: Refs #1442
growth: +68 active-main ancestry suffix and ordered console append; weighed deleting the suffix and reconstructing hidden parent IDs in the client, which lacks the worker's saved entries
raise: crate acp +56, crate console +8, crate types +4
raise: tests +148
ledger+: console right-click menu and pane close mark | live committed notices retain ordered active-main ancestry, including hidden model-change and tool-result entries
ledger+: ACP v2 server | `_yi/committed` sends the ordered active-main ancestry suffix through each saved message
---
A live committed message now carries every active-main entry after the prior commit cursor, oldest first, alongside the saved message. The console appends those ancestors before the message while retaining replay/live deduplication, so hidden ModelChange and ToolResult entries keep the visible tool call on the active path and Up/Enter rewind to that selected call.
