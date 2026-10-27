---
issue: Refs #1673
growth: +185 the closed `final` event (kinds, quota window and reset), exit 4, a `final` line from the early refusals, and the schema-failure kind; weighed: no new ErrorClass variant (a Provider("quota") value serves), no regex dependency for the reset instant, and one shared refuse/early_final pair instead of a print at each of six returns
raise: tests +177, comments +5, crate cli +181, crate types +4
---

A quota refusal (`HTTP 429 quota exhausted`) now classes as `provider:quota` instead of plain transport http, so the loop no longer re-sends it with a stream-retry steer. `yi ask --json` ends every run with one `final` event whose `kind` is `stop`, `no_answer`, `deadline`, `timeout`, `quota` or `error`; a quota final also carries the window (`5h`, `7d` or null) and the first ISO-8601 reset instant found in the provider's message, and the run exits 4 in json and plain mode alike.
