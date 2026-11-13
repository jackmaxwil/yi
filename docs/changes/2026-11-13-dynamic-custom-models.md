---
issue: Closes #2201
decision: Custom providers with no explicit models discover their OpenAI-compatible model list on first use | Static model lists remain offline and unchanged, while an unavailable dynamic provider does not delay unrelated startup | Configure explicit models or restore the previous catalog-only resolver
growth: +300 weighed fetching inside every catalog lookup and a process-global error slot; one shared fetch serves a listing, a stale list still resolves, and a failed fetch is tried again after 30 seconds
raise: crate cli +11, crate runtime +242, crate types +47, tests +248
ledger~: custom providers | declares an endpoint and its models | declares an endpoint and, with an empty or omitted model list, discovers its models from `/models` on first use
---

A custom provider can now omit `models` or leave it empty. Yi fetches its OpenAI-compatible `/models` response when the provider is first selected or listed, caches the result for the process, and applies any advertised context, output, and image-input capabilities. Explicit model entries continue to work without network discovery.
