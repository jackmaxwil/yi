---
version: 0.913.0
decisions: D544
issue: Closes #1514
decision: a server wait past the retry cap, or `x-should-retry: false`, ends the turn stamped `retry refused:` instead of falling back to backoff, `x-should-retry: true` retries any status, and the loop reads that stamp, a quota and a refusal as final verdicts however much output they billed (revises the YI_DESIGN retry bullet) | a `retry-after: 61` was slept out and the same bytes sent again, and a refusal that billed output tokens was re-run with a steer saying the stream had dropped | restore the backoff fallback in `retry::wait_for` and drop the verdict gate in `stream_retry::nothing_delivered`
ledger+: pure loop + event enum | a 4xx other than 408/409/429 refuses the same bytes again, so the loop spends no second request and no 99-byte steer on one (`loop_events::a_400_body_ends_the_turn_with_no_retry_and_no_steer`), while a routing 404 may be resent once, an unused budget ends a backoff before its request rather than after it (`provider_stream::an_interrupt_during_a_backoff_sends_no_further_request`), and a retry-after past the cap ends the turn naming the wait (`provider_stream::a_wait_past_the_cap_ends_the_turn_and_one_inside_it_retries`)
growth: +308 classing provider errors once, pooling the connections a turn hands back, keeping the partial turn every adapter had already been billed for, and keeping leak recovery off quoted text
raise: crate ai +265, crate loop +13, crate types +30, tests +830, comments +32
---
Provider errors are classed once, the connection a finished stream hands back is pooled, and a
turn that dies mid-stream keeps what it produced.

`ErrorClass::is_transport` holds for `TransportHttp` only on 408, 409, 429 and 5xx, so a 400,
401 or 403 is no longer re-sent with the 99-byte "the same turn runs again" steer; a routing 404
may still be resent once, because a follow-up drops the forced choice that caused it
(`ErrorClass::is_wire_refusal`). A quota 429 classes as the typed `provider:quota`, and a
refused retry and a refusal as provider verdicts; none is re-run, whatever output it billed. `retry_delay` returns a `Duration`; a server wait
past the policy's cap ends the turn naming the wait instead of sleeping it out and sending again;
`x-should-retry` overrides the status; the backoff sleeps in 100 ms slices, so a cut ends the wait
instead of sending the request the loop no longer wants.

A stream that stops at `response.completed` now drains its body to EOF, handing the connection back
to ureq, and the pooled agent keeps 16 idle connections per host instead of ureq's one. A keepalive
`data:` line is skipped on both wires; the drain reads on a thread the turn waits two seconds for,
so a body that never ends costs the turn no more than that. A UTF-8 split keeps only an incomplete
tail of at most three bytes, and an invalid byte ends the stream naming its offset rather than
stalling to the read timeout.

Anthropic and Responses fail the running mapper on a mid-stream error, as chat does, so the text,
tool calls and usage a cut turn produced survive in its error turn. A refusal ends the turn as
`The model refused: …` instead of an empty stop. Responses keeps one thinking block per reasoning
item and opens a new text block after a function call.

Leak recovery leaves a block inside a fence or an inline span as text, leads its ids with the
message's own clock so a resumed session cannot mint `leak-0` again.
