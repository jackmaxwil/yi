---
version: 0.965.0
decisions: D559
issue: Refs #748
raise: crate ai +201, crate runtime +66, crate types +14, tests +421, comments +11
growth: +288 (measured against the fork): the tail drill in `cache_miss`, the strip-retry and its record in `request`, the tail flag through `LlmContext`/`CachePolicy`/`Breakpoints`, and their regression tests; weighed for deletion: `TtlEstimate` and its `estimate()` accessor are replaced by `CacheAdvice`/`advice()`, and upstream routing is left unchanged
decision: a route's moving tail mark is suspended for an hour after four consecutive unexplained requests that wrote and read nothing back, and a 400 naming a cache field is retried once with only the encoder's cache fields stripped | the session ledger already shows both failures, so no model, probe, stored state or outbound routing change is needed | remove the tail escalation in `cache_miss::MissTracker::escalate` and the strip branch in `request::send_stripping_cache`
ledger=: cache marks and TTL from the session ledger | 6, 7 | core | M | M | a loop request's history marks and their TTL come from the session's own replies: the TTL remains the expected-cost choice D315 landed (`cache_miss::the_cheapest_ttl_tracks_the_better_static_ttl_on_known_gap_mixes`, T0), four unexplained requests that wrote and read nothing back stop the moving tail mark on that route for an hour (`cache_miss::four_zero_read_writes_stop_the_tail_mark_for_an_hour`, `openrouter::a_suspended_tail_mark_keeps_the_marks_before_it_only`, T0), and a 400 naming a cache field is resent once with only the encoder's cache fields stripped, the field recorded on the reply (`strip::a_400_naming_a_cache_field_is_resent_once_without_it`, `strip::the_reply_records_the_field_the_route_refused`, T0)
---

A route's cache marks now follow what its own session shows (Refs #748). Two behaviours build on the ledger fold D315 landed, and neither asks a model nor changes provider routing.

The tail mark: a request's own tail is where the next request reads from, and a route that wrote it and read nothing back for four consecutive unexplained requests is writing a mark nothing reads. The fold stops the moving tail on that route for an hour and keeps every mark before it, the previous tail included, since those are what the next request does read. Requests during the suspension do not re-arm it. After the hour the mark is written once more: four more zero-read writes suspend it again, a read ends the run and the mark stays.

The refusal: a provider that does not know a cache field answers 400 rather than ignoring it. A 400 whose body names one of `cache_control`, `cache_ttl`, `prompt_cache_key`, `prompt_cache_retention`, `session_id` or `cache` is asked again once with only the cache fields the encoder wrote removed from the body. Fields inside tool schemas, tool calls and tool results stay unchanged, and the reply records the refused field as a `cache_stripped` diagnostic. A 400 that names none of them is reported unchanged, never resent.

The previous tail needed no switch of its own: D295's build already sends it on every loop request, so the escalation it was meant to gate is unconditional.

The TTL-expiry escalation ladder remains open under #748. This PR keeps D315's cost-minimising TTL choice and does not close that issue.
