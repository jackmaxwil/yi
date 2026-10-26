---
issue: Closes #1500, Refs #1442
raise: crate tui +3
---
This is Bet 1 items 4 and 7 of Yi's next shape (B1-loud-returns): every cut of a child's return now says what it cut.

Four places shortened what a child sent its parent and said little or nothing about it. A `context_keys` value past 4,096 chars ended `… [truncated to 4096 chars]`, with no total and no road to the rest. The whole scope past 16,384 chars was cut the same way. The preview of a child's answer, which rides the completion notice's `Last answer:` line and the status the TUI card reads, was cut at 240 chars with a bare ellipsis. And a schema refusal (`rlm.result` of a child that answered prose where a schema was given) embedded the child's whole answer, however long. The first three now end in one row, `… [kept N of M chars · cap <name> = N · <call>]`, written by the one `clamp` in `mailbox.rs` that every cut of a child's return goes through, and the fourth is bounded by it.

- A value past `mail.context_value` names the way to pass it whole: `write a value to a file and give local://<path> in partition`. The scope past `mail.context_total` names the same way. The kernel cuts each value at the cap plus one, so the host cannot know its full size and the row says `over 4096`, never a length it did not count. Cuts of text the host holds whole carry their exact total.
- The answer preview names `PREVIEW_CHARS = 240` and `read history://<name>`, which resolves for a live child and a reaped one alike; a test reads the address off the row and fetches it both ways.
- A schema refusal, and the refusal of a checked child whose answer is no result object, carry at most `RESULT_TAIL_CHARS` (2,000) chars of the answer and name `history://<name>` for the rest; a test fetches that address too.

The TUI reads a child's preview as the head of the answer it is already streaming and replaces its card's text only when that head is not found there. The cut row is no part of the answer, so the match stops at the last `CUT_ROW` in the preview. That is the one opener the runtime writes every cut row with and the card matches on, so the two cannot drift apart, and an answer holding a `… [` of its own in its first 240 chars is not cut short by it. Without the match, every long answer's card would have shown the 240-char preview and its row in place of the streamed text until the child finished. The change is a few lines in `tui/src/app/children.rs`; a TUI test streams a long answer and then delivers the cut preview, and streams `Done` and then delivers an uncut preview with a `… [` in it.

The tests cover the context value at and one past its cap, a non-ASCII value and one whose byte 4,096 sits inside a two-byte char, the whole scope at and one past 16,384, the remedy read off a cut value's row and run through a partition, schema refusals at 2,000, 2,001 and 10,000 chars, and answer previews at 240 and 241 chars. Their fixtures have the shape the kernel's `_resolve_context` produces.

The feature-ledger row for the kernel and `rlm()` subagents says it, and names the runtime tests among its journeys.
