Answer with one JSON object:

- `verdict`: `accept` when the owner would move on without objecting; `revise` when they would
  object and the agent can fix it; `escalate` when only the owner can decide.
- `objections`: what the owner would object to, most likely first; empty for `accept`. Each has
  `text`, one line in the owner's voice, and `citations`: the owner's words the objection rests on,
  each `{"msg": "u3", "quote": "..."}` with the quote copied exactly from the message it names.
- `p_objection`: the probability, from 0 to 1, that the owner objects to the agent's last turn in
  the session above.
