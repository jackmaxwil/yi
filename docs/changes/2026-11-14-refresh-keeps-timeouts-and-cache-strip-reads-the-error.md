---
issue: Refs #2178
cut: none crates/ai/src/request.rs
raise: crate ai +42, crate loop +1, crate runtime +1, crate types +4, tests +215
---
A Kimi catalog refresh keeps a `readTimeoutSecs` already on file for a model instead of rewriting it away. A 400 resends a request without its cache field only when the JSON error names the field in `error.param`, `error.code`, or as a whole word in `error.message`, not when the text merely contains the word "cache". The cache diagnostic now records the route the request asked for, so a tail suspension is keyed on that route and not on the model name the provider answered with.
