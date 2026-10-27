---
issue: Refs #1673
growth: +233 the closed `final` event (kinds, quota window and reset), exit 4, a `final` line from the early refusals, the schema-failure kind and the hold-expiry test; weighed: no new ErrorClass variant (a Provider("quota") value serves), no regex dependency for the reset instant, and one shared refuse/early_final/error_final trio instead of a print and a class spelling at each of six returns
raise: crate ai +1, crate cli +224, crate types +8, tests +234, comments +6
---

A quota refusal (`HTTP 429 quota exhausted`) now classes as `provider:quota` instead of plain transport http, so the loop no longer re-sends it with a stream-retry steer. `yi ask --json` ends every run with one `final` event whose `kind` is `stop`, `no_answer`, `deadline` (only when `--deadline` fired), `timeout`, `quota` or `error`, and whose `class`, on an error, is an `ErrorClass` spelling such as `refusal:unknown_model` or `harness:schema`; a quota final also carries the window (`5h`, `7d` or null) and the first ISO-8601 reset instant found in the provider's message, and the run exits 4 in json and plain mode alike.
