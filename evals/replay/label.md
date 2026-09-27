You label one recorded moment. The agent ended a turn, the owner wrote the next message, and the
agent replied to it. Classify the owner's next message:

- `objected`: the owner corrects, rejects, or complains about the turn.
- `check_revealed`: the owner asks a check question, in their words "i ask ai if the plan is done
  and everything was done correctly", and the agent's own reply admits something was missed or
  wrong.
- `accepted`: the owner continues, approves, or moves on to new work.

Answer with one JSON object: `label`; `objection`, one line naming what the owner objected to
(empty for `accepted`); and `quote`, the owner's words that show it, copied exactly from the next
message (empty for `accepted`).
