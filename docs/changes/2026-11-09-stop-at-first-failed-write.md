---
raise: crate loop +14, comments +1
---
A message's tool calls now run from their own module, `crates/loop/src/batch.rs`, beside the turn loop rather than inside it; nothing about how they run has changed.
