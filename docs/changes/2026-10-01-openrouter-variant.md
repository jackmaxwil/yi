---
issue: Closes #978
raise: crate runtime +23, tests +15, comments +1
---
An OpenRouter routing variant resolves (Closes #978). `--model openrouter/z-ai/glm-5.3-flash:exacto` was refused as an unknown model, because OpenRouter's model list names only base ids. `resolve_model` now gives `<base>:<variant>` on OpenRouter the base entry's limits and prices, and sends the full id on the wire. Other providers keep exact matching, since a colon can belong to their ids.
