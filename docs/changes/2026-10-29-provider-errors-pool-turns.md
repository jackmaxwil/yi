---
issue: Closes #1514
growth: +279 classing provider errors once, pooling the connections a turn hands back, keeping the partial turn every adapter had already been billed for, and keeping leak recovery off quoted text
raise: crate ai +235, crate loop +7, crate types +37, tests +652, comments +30
---
Provider errors are classed once, the connection a finished stream hands back is pooled, and a
turn that dies mid-stream keeps what it produced.

`ErrorClass::is_transport` holds for `TransportHttp` only on 408, 409, 429 and 5xx, so a 400,
401 or 403 is no longer re-sent with the 99-byte "the same turn runs again" steer; a routing 404
may still be resent once, because a follow-up drops the forced choice that caused it
(`ErrorClass::is_wire_refusal`). Overflow bodies class as `provider:overflow` and a quota 429 as
the typed `provider:quota`, both non-retryable. `retry_delay` returns a `Duration`; a server wait
past the policy's cap ends the turn naming the wait instead of sleeping it out and sending again;
`x-should-retry` overrides the status; the backoff sleeps in 100 ms slices, so a cut ends the wait
instead of sending the request the loop no longer wants.

A stream that stops at `response.completed` now drains its body to EOF for at most two seconds, so
ureq hands the connection back to its pool, and the pooled agent keeps 16 idle connections per host
instead of ureq's one. A UTF-8 split keeps only an incomplete tail of at most three bytes, and an
invalid byte ends the stream naming its offset rather than stalling to the read timeout; an
unparsable data line ends the turn on the chat and Responses wires as it always has on Anthropic.

Anthropic and Responses fail the running mapper on a mid-stream error, as chat does, so the text,
tool calls and usage a cut turn produced survive in its error turn. A refusal ends the turn as
`The model refused: …` instead of an empty stop. Responses keeps one thinking block per reasoning
item and opens a new text block after a function call.

Leak recovery leaves a block inside a fence or an inline span as text, numbers its ids per message
so two messages never share `leak-0`, and is no longer called on the Responses mapper.
