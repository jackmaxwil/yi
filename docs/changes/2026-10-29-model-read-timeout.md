---
issue: Closes #1824
growth: +205 deadline plumbing through three adapters, the refresh merge and the loop guard; the fixture dedupe in tests already deleted what it could
decision: a catalog entry's `readTimeoutSecs` (default 60 s) is the model's own read deadline, applied per read so it bounds the idle gap between chunks and never the whole body, and a timeout is neither retried nor resent | four commands ended mid-task on a gateway that answers only when its first token is ready, the fixed 60 s was the HTTP client's and named nothing, and the retry layers spent it three times over | delete the field from the catalog entry, which restores the client's constant
raise: crate ai +131, crate cli +42, crate tui +1, crate types +36, comments +12
---
A run no longer dies on the wait for its first byte without being told what to change. `Model` gains
an optional `readTimeoutSecs`, and the stream request takes its read deadline from the model rather
than a constant in `yi-ai`: the bundled, cached and custom catalogs all reach it through the same
serde shape, so an entry written by hand or by a cache refresh carries it with no other change. The
deadline bounds one read, so a generation that keeps emitting chunks outlives its value and only a
silence longer than it does not. A read timeout is neither retried by `send_with_retry` nor resent
by `pump_sse_with_resend`, because either layer doubled a stall the field had already bounded; the
refusal says it timed out reading response after N s, names `readTimeoutSecs` and its 60 s default,
points at the catalog entry, and states how many attempts ran. A `yi ask` run that ends on a
provider error now exits 1 in `--json` mode too, where it used to exit 0 with the error only in the
final line.

The loop does not re-run a turn that ended on that refusal, so one stall costs one deadline, and a
catalog refresh keeps a `readTimeoutSecs` already in the cached entry instead of rewriting the file
without it.
