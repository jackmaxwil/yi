---
issue: Refs #2204
raise: crate types +1, tests +12
---

A discovered model now carries the `reasoning` flag its provider advertises. Before, discovery set every model to non-reasoning, so a configured thinking level was clamped to off and `reasoning_effort` never reached the gateway, even for a relay that lists `high` as a supported level.
