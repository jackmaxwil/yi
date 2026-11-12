---
version: 0.1034.0
decisions: D588
issue: Closes #2144
raise: crate cli +1, crate types +36, tests +73
cut: none docs/YI_DESIGN.md
shares: #2135 the GitHub #5 import; both edit other paragraphs of docs/YI_DESIGN.md
shares: #2136 the GitHub #6 import this splits out; its OpenRelay edits to anthropic.rs and §5 are what this replaces
shares: #2137 stacked on the #6 import, so it carries the same OpenRelay edits to anthropic.rs and §5
shares: #2139 the GitHub #9 import; its composer keys and this api field are different keys in config.rs and the design
shares: #2150 the tab-title lane from the same #6 import; it edits other paragraphs of docs/YI_DESIGN.md
shares: #2152 the identicon lane from the GitHub #4 import; it edits other paragraphs of docs/YI_DESIGN.md
shares: #2153 the hard-link lane from the same #6 import; it edits other paragraphs of docs/YI_DESIGN.md
shares: #2138 the GitHub #8 import; its theme field and this api field are different keys in config.rs and the design
decision: a custom provider in `providers` names the wire it speaks with an optional `api` (`openai-completions` when unset, `openai-responses`, `anthropic-messages`; any other value refuses at load), and a custom Messages endpoint gets its key as `authorization: Bearer` through `compat.bearerAuth`, never `x-api-key` (amends D562) | GitHub #6 asked for OpenRelay, a gateway on the user's own machine serving OpenAI responses and Anthropic messages with one bearer key, as a built-in provider with an empty bundled catalog whose models lived in the refresh cache; a field on the existing custom provider reaches the same gateway, and any other, with no vendor id reserved in yi-types and no hand-written cache file that doctor reads as stale | delete `api` and `CustomApi`, so every custom provider speaks completions again and a config naming `api` refuses as an unknown field
ledger-: custom OpenAI-compatible providers
ledger=: custom providers | 5 | core | M | L | `providers` in `~/.yi/config.json` declares an endpoint and its models, each addressed `<id>/<model>`; the endpoint speaks chat completions unless `api` names `openai-responses` or `anthropic-messages`, a Messages endpoint taking the key as a bearer token as the OpenAI wires do, and each wire appends its own path to `baseUrl` (`/chat/completions`, `/responses`, `/v1/messages`); merged after the bundle and the catalog cache, so a refresh neither rewrites nor fetches it; the key is `apiKeyEnv`, else a literal `apiKey`; read once per process, so an edit takes hold on restart; a built-in id refuses at load; `yi doctor` names each, its api and its key's source
---
A provider declared under `providers` in `~/.yi/config.json` can now speak OpenAI responses or
Anthropic messages, not only chat completions. `"api": "anthropic-messages"` sends its requests
to `<baseUrl>/v1/messages` with the key as `authorization: Bearer` and `anthropic-version`, and
`"api": "openai-responses"` to `<baseUrl>/responses`; with no `api` it is the completions
provider it was. A misspelled value refuses at load, naming the three that exist. Since each
wire appends its own path, a Messages `baseUrl` stops before `/v1` and a Responses or
completions one ends in it. `yi doctor`'s `providers` row now names each provider's api.

This is what GitHub #6 needed from its OpenRelay provider: a local gateway serving both routes
behind one bearer key. That change named the gateway in yi-types, shipped it an empty catalog and
read its models from the refresh cache; a custom provider per route does the same with nothing
reserved, since `api` is per provider: one entry with `"api": "openai-responses"` and one with
`"api": "anthropic-messages"`, both naming the same key variable. Credit: Max Ludwick found the gap and wrote the first version.
