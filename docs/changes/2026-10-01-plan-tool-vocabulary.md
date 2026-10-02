---
issue: Closes #952
raise: crate runtime +5, tests +157
---
The plan tool is taught in its own field names (Closes #952). The orchestrate protocol told a model that every task carries a title, an acceptance, a check and deps, and doctrine said to lift todos "with a check per task"; the tool takes `label`, a `contract` whose items carry `decider: {cmd}`, and `after`, so a model spent five refused `init` calls learning the names from the errors. The protocol and doctrine now name the tool's fields, and a checklist with nothing for the engine to run is one `op=set`; the `todos` schema types each todo (`label` at most 80 characters, `after` and `intent` as lists, a contract item's `id`, `critical`, `weight` and `decider` required) instead of one 900-character sentence; and a refusal of `check`, `acceptance`, `accept`, `title`, `deps`, `after` or `intent` says where the field belongs.
