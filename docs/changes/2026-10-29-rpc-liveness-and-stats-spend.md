---
issue: Closes #1542
raise: crate cli +95, crate types +8, tests +297
---
`yi stats <id>` billed only the messages it could fold, so a family's child lanes
and a session's side calls, which book their spend as `usage` records and never
as entries, went missing from the row: `--since` reads each child's own file and
is right, but the single-session view was short wherever a child or a title,
select, compact or branch call had spent. The fold now reads the ledger's
`billed_usage` rows too, the same predicate the store bills its own `SessionStats`
with, so `yi stats` and `yi rpc`'s `get_session_stats` report one number; a
`cause: "assistant"` row mirrors an entry and still bills nothing.

`yi rpc` answered every command inside its `select!`, so while `compact` awaited
a summary the event arm was parked: a turn streaming into the broadcast buffer
over it lost frames the client was never told it lost (`event_gap`), and a state
query sent meanwhile answered only once the command ahead of it had finished.
Each command now answers from its own task against shared state, so the loop
never stops reading events and a reply lands in the order its command finished.
Three Pi compatibility stubs are honest with it: `isCompacting` reports a compact
in flight instead of a constant false, `get_commands` lists the runtime's slash
verbs instead of an empty array, and `get_state` no longer echoes
`steeringMode`/`followUpMode` values nothing reads, their setters answering with
the reason instead.
