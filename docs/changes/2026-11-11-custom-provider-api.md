---
issue: Closes #2144
raise: crate types +25, tests +72
decision: a custom provider in `providers` names the wire it speaks with an optional `api` (`openai-completions` when unset, `openai-responses`, `anthropic-messages`; any other value refuses at load), and a custom Messages endpoint gets its key as `authorization: Bearer` through `compat.bearerAuth`, never `x-api-key` (amends D562) | GitHub #6 asked for OpenRelay, a gateway on the user's own machine serving OpenAI responses and Anthropic messages with one bearer key, as a built-in provider with an empty bundled catalog whose models lived in the refresh cache; a field on the existing custom provider reaches the same gateway, and any other, with no vendor id reserved in yi-types and no hand-written cache file that doctor reads as stale | delete `api` and `CustomApi`, so every custom provider speaks completions again and a config naming `api` refuses as an unknown field
ledger~: custom OpenAI-compatible providers | declares an endpoint that speaks chat completions and its models | declares an endpoint and its models; the endpoint speaks chat completions unless `api` names `openai-responses` or `anthropic-messages`, a Messages endpoint taking the key as a bearer token like the other two
---
A provider declared under `providers` in `~/.yi/config.json` can now speak OpenAI responses or
Anthropic messages, not only chat completions. `"api": "anthropic-messages"` sends its requests
to `<baseUrl>/v1/messages` with the key as `authorization: Bearer` and `anthropic-version`, and
`"api": "openai-responses"` to `<baseUrl>/responses`; with no `api` it is the completions
provider it was. A misspelled value refuses at load, naming the three that exist.

This is what GitHub #6 needed from its OpenRelay provider: a local gateway serving both routes
behind one bearer key. That change named the gateway in yi-types, shipped it an empty catalog and
read its models from the refresh cache; a custom provider per route does the same with nothing
reserved. Credit: Max Ludwick found the gap and wrote the first version.
