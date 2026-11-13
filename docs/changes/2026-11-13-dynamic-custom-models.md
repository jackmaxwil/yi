---
issue: Closes #2201
decision: Custom providers with no explicit models discover their OpenAI-compatible model list on first use | Static model lists remain offline and unchanged, while an unavailable dynamic provider does not delay unrelated startup | Configure explicit models or restore the previous catalog-only resolver
growth: +151 weighed dropping the proxy read on the discovery GET; a fetch that ignores HTTPS_PROXY fails as an unknown model for anyone whose completion already needs that proxy
raise: crate cli +5, crate runtime +99, crate types +47, tests +59
ledger~: custom providers | declares an endpoint and its models | declares an endpoint and, with an empty or omitted model list, discovers its models from `/models` on first use
---

A custom provider can now omit `models` or leave it empty. Yi fetches its OpenAI-compatible `/models` response when the provider is first selected or listed, caches the result for the process, and applies any advertised context, output, and image-input capabilities. Explicit model entries continue to work without network discovery.
