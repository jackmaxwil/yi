---
raise: crate ai +29, tests +95
ledger=: OpenRelay catalog and auth | 5 | core | M | L | user overlays resolve both routes; OpenRelay Anthropic uses Bearer while ordinary Anthropic retains x-api-key
---
OpenRelay is now a catalog provider with an empty bundled catalog; its model entries come from the user's `~/.yi/catalog/openrelay.json` overlay. The Claude entry is unpriced, not free. The GPT entry records input at $0.20/M, output at $1.20/M, and cache-write at $0.25/M; cache-read price is unobserved.

Contract tests cover both model routes and the Anthropic authorization split without network requests.
