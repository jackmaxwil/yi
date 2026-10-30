---
issue: Closes #1761
raise: tests +65, comments +2, crate context +64, crate runtime +4
---
A compaction summary that quotes a user message by address (`user://3: "…"`) is now checked against that message. The summarizer's quote is compared, whitespace aside, with the typed message the address names; a quote that message does not contain, or an address no typed message has, gets ` [unverified: not in user://N]` after it in the summary the model reads. Before, nothing verified the quotes `prompts.rs` asks for, so a tool result carrying a planted "user said: …" line could come back after compaction as the user's own goal. The marker is written into the compaction view only: the session ledger is unchanged, and a summary produced without the session's user index (no store) is left as written.
