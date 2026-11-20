---
issue: Closes #2178, Closes #2215
decision: a request on a pooled socket that has already carried a finished stream is bounded by the model's read deadline on a helper thread, ends with the usual timeout refusal and drops the agent's pooled sockets, and is never sent a second time; a child's model keeps its `readTimeoutSecs` or 300 s (amends D557) | ureq clears the read deadline when a socket returns to its pool, so a request written into a socket a NAT dropped waited forever; a resend would bill a slow but live socket twice; a Sonnet child writing a design document died at the 60 s default | reversible via deleting `send_on_pooled` and `CHILD_READ_TIMEOUT_SECS`, which restores D557
ledger~: model read deadline | a timeout is neither retried nor resent, its refusal | a timeout is neither retried nor resent, a pooled socket silent past it ends the same way and is dropped from the pool, a child keeps 300 s unless its entry says otherwise, its refusal
raise: comments +7, crate ai +100, crate runtime +3, crate types +6, tests +236
cut: none crates/ai/src/request.rs
shares: #2297 it edits the provider and timeout lines of YI_DESIGN.md; this adds one clause to the read-timeout bullet and one key to the providers line
shares: #2295 same file; different bullets
shares: #2293 it edits request.rs; this moves the pool functions to request/pool.rs and edits send_stripping_cache's send call
shares: #2256 it edits config.rs; this adds one field to CustomModel and its catalog line
shares: #2239 same file YI_DESIGN.md; different bullets
shares: #2237 it edits the build and family harness; this adds a read-timeout default in cast and one Built field
shares: #2205 it edits custom_providers.rs and config.rs; this adds one test and one CustomModel field
---
A request written into a pooled connection that a NAT had silently dropped used to wait on it with no deadline, because the HTTP client clears the read timeout when a socket returns to its pool. Such a request is now timed on the model's `readTimeoutSecs` on a helper thread (at most 16 at once, each ending when the kernel gives its socket up) and, silent past it, ends with the usual timeout refusal and drops the pool, so the next request dials anew; the prompt is never sent a second time, since it may have landed. A model declared under `providers` can now carry `readTimeoutSecs`, which reaches its catalog entry. A child session's model waits 300 s for one read unless its entry sets its own. The remaining review findings of #2178 were already fixed on main (the Kimi refresh keeps the field, the cache-strip retry reads the error's `param`, `code` and message words, the tail suspension keys on the requested route); the missing-credential sentence for a provider with no `apiKeyEnv` now has a test, and a read already lifted the tail suspension under test.
