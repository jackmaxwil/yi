---
issue: Closes #1892
decision: a second question on the classifier sidecar, consumer `inject`, marks the text it names as addressing the agent and never withholds it | the pilot separated subtle injections at a 0.8 cut with one false alarm in twelve hard ordinary texts, and a false positive that withheld a page would cost the answer the page carried, so the fence carries the score and lets the model weigh it | drop `classifier::Screener` and the `screen=` field from both fences
growth: +205 the screen's one question on the classifier client already in `classifier.rs`, its two config keys, the source kind each external attach names, the mark carried through the yard's snapshot, and four tests; weighed for deletion and not taken: a `screen` module of its own (the sidecar clone, the breaker and the journal hook all live in `classifier.rs`, so it would be the same bytes plus a seam), and a cap row built at each of the four call sites instead of once in the screen (four copies of one sentence, and the loud row is the screen's own fact)
raise: crate runtime +201, crate types +4, tests +154, comments +7
---
The classifier sidecar now answers a second question, `inject`, on text that crossed in from
outside: attached project and memory files, web search results and a reviewed patch. A score at
or above `classifier.screenAt` (0.8 when unset) writes `screen="flagged:0.93"` onto the fence
header and journals a `classify` row under consumer `inject` carrying the source kind, the text's
hash and the action; the text itself is kept, so the model reads the page it was sent and can see
why it is marked. A clean text journals nothing. A call that fails or times out passes the text
unmarked and records `unscreened`. Past `classifier.screenMaxChars` (96 000) the screen reads the
head and the fence opens with a `[…]` row naming the chars it read of the total, the cap, and
that the rest was not checked.
