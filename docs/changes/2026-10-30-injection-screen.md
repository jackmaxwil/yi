---
issue: Closes #1892
decision: a second question on the classifier sidecar, consumer `inject`, marks the text it names as addressing the agent and never withholds it | the pilot separated subtle injections at a 0.8 cut with one false alarm in twelve hard ordinary texts, and a false positive that withheld a page would cost the answer the page carried, so the fence carries the score and lets the model weigh it | drop `classifier::Screener` and the `screen=` field from both fences
growth: +467 the screen's one question on the classifier client already in `classifier.rs`, its two config keys, the source kind each external attach names, the mark carried through the yard's snapshot, the session's own screen slot, the cooldown and the per-text cache, the host's parked-attach queue and the run task's one drain of it, and six tests; weighed for deletion and not taken: a `screen` module of its own (the sidecar clone, the breaker and the journal hook all live in `classifier.rs`, so it would be the same bytes plus a seam), a second breaker beside the one `classifier.rs` already had (two walkers of one state machine that had already diverged on the poison path, so `paused` now carries the cause and answers paused on a poisoned lock), and a cap row built at each of the four call sites instead of once in the screen (four copies of one sentence, and the loud row is the screen's own fact)
raise: crate runtime +463, crate types +4, tests +610, comments +38, over-cap +1
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

The screen belongs to the session that owns it. A second session in the same process starts
with none, so nothing inherits an endpoint, a key or a cut from the session attached before it.
The request carries only the head the cap allows, while the model reads the whole text under
its `[…]` row. A failed or timed-out call opens a 60-second cooldown, and later checks in that
session skip the call and record `unscreened` with the cause it failed for; a text already
screened in the session is not sent a second time.

Attached text is screened where the turn can wait for it: the extension host parks each untrusted
attach, and the run's own task drains the park on a blocking thread before the first request is
assembled, so a hung endpoint costs one blocking thread rather than the session's worker or the
host's lock.
