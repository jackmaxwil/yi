---
issue: Refs #2204
raise: crate runtime +5, tests +12
---

The picker credential filter now shares one rule between the console and TUI pickers and the subagent model list, and a discovered model's `max_output_tokens` no longer fails the whole model list when a gateway sends it as a non-number.
