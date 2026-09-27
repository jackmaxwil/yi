You label one recorded moment. The agent ended a turn, the owner wrote the next message, and the
agent replied to it. You also see the owner's earlier messages, each under an id `[u1]`..`[uN]`; a
`[…]` row marks where a cap cut them, and nothing can fetch what it cut.
Classify the owner's next message:

- `objected`: the owner corrects, rejects, or complains about the turn.
- `check_revealed`: the owner asks a check question, in their words "i ask ai if the plan is done
  and everything was done correctly", and the agent's own reply admits something was missed or
  wrong.
- `accepted`: the owner continues, approves, or moves on to new work.

For `objected`, `kind` says what the objection rests on:

- `intent_loss`: something the owner already said in `[u1]`..`[uN]` (an instruction, a name, a
  scope, a choice) that the turn dropped, changed or contradicted.
- `new_info`: a new requirement, an opinion or a taste that the earlier messages do not state.

Answer with one JSON object: `label`; `kind`, empty unless `objected`; `objection`, one line
naming what the owner objected to (empty for `accepted`); `quote`, the owner's words that show
it, copied exactly from the next message (empty for `accepted`); and `rests_on`, for
`intent_loss` the earlier words it rests on, each `{"msg": "u3", "quote": "..."}` with the quote
copied exactly from the message it names, else empty.
