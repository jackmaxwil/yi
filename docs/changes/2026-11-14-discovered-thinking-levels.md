---
issue: Refs #2204
shares: #2220 crates/acp/src/lib.rs comes from the #2205 commits this branch is stacked on, not from this change
raise: crate types +22, tests +33
---

Discovery now reads a relay's `reasoning` flag and `variants` table. A model that lists `xhigh` and `max` (OpenRelay's `bedrock-claude-opus-5-5`) offers both in the thinking picker and sends the level the relay names. A level the table omits is unsupported for that model. Before, every discovered model was marked non-reasoning with no level map, so the picker stopped at high.

A custom provider's hand-written model entry also takes `reasoning` and `thinkingLevelMap` now, so the same levels can be declared by hand.
