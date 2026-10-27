---
issue: Refs #1673
raise: tests +85, comments +2, crate cli +111, crate types +4
---

A quota refusal (`HTTP 429 quota exhausted`) now classes as `provider:quota` instead of plain transport http, so the loop no longer re-sends it with a stream-retry steer. `yi ask --json` ends every run with one `final` event whose `kind` is `stop`, `no_answer`, `deadline`, `timeout`, `quota` or `error`; a quota final also carries the window (`5h`, `7d` or null) and the first ISO-8601 reset instant found in the provider's message, and the run exits 4 in json and plain mode alike.
