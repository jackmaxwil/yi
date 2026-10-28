---
issue: Closes #1706
growth: +859 provider transport and typed DTOs, validation and untrusted rendering, credential/config wiring, once-only accounting, and small TUI/ACP mappings; existing HTTP, usage parsing, fencing and cut helpers were reused, while caching, recency, a second provider and custom UI states were weighed and omitted
decision: Web search uses a provider-generic side completion, with models.webSearch defaulting to openrouter/deepseek/deepseek-v4-flash and webSearch.engine defaulting to exa | the recorded DeepSeek calls were faster and cheaper than GPT-5 Nano with the same five citations, and a side completion works with every primary provider | set models.webSearch to another OpenRouter model, or remove the built-in search default
decision: Web search permission: "Never ask" | the tool is a read in every permission mode, with irreversible left false | make the tool irreversible and restore permission prompts
decision: Live test spend: "Yes, up to $1" | six recorded OpenRouter calls cost $0.049 and supply the parser fixtures | keep future live smoke tests opt-in and outside every gate
raise: crate ai +487, crate cli +50, crate runtime +307, crate tui +4, crate types +11, tests +790
---
Add a credential-gated `web_search` tool backed by OpenRouter's server-side search, defaulting to Exa. Return cited answers as untrusted data, preserve measured token and dollar usage, and make the tool available independently of the primary model provider.
