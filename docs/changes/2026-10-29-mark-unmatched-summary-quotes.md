---
version: 0.925.0
issue: Closes #1761
ledger+: context P1–P19; prefix-aligned P7 | a summary quote that the `user://<n>` message it cites does not contain, as typed or as its key row escapes it, is marked `[unverified: not in user://<n>]` in the summary the compaction entry stores, the cited messages untouched (`compaction_faux::a_summary_quote_the_addressed_message_lacks_is_marked_unverified`, T1)
raise: tests +78, comments +2, crate context +70, crate runtime +4
---
A compaction summary that quotes a user message by address (`user://3: "…"`) is now checked against that message. The summarizer's quote is compared, whitespace aside, with the typed message the address names; a quote that message does not contain, or an address no typed message has, gets ` [unverified: not in user://N]` after it in the summary the model reads. Before, nothing verified the quotes `prompts.rs` asks for, so a tool result carrying a planted "user said: …" line could come back after compaction as the user's own goal. A quote copied from the key row's escaped form (`\n`, `\"`) or wrapped onto the next line is checked too. The marker is part of the summary the compaction entry stores; the cited user messages are unchanged, and a summary produced without the session's user index (no store) is left as written.
