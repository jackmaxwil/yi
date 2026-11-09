---
issue: Closes #2105
---
The dist build compiles `regex-automata` at opt-level 3, in the per-package form `yi-orb` already uses. Resuming a 147-entry session in the TUI retires 3,887.7M instructions at "z" and 2,297.1M at 3 (-40.9%, 12 interleaved runs), for 148,608 bytes of binary. `ask` retires 6.2% fewer (154.1M to 144.5M), a gain that comes from the redact regexes compiling eagerly at startup and is not specific to this change's target path. `fancy-regex` or `syntect` at 3 instead cost 33,024 bytes and gain -0.5% and +0.3%, so the regex engine they sit on is the lever. The size-ledger header now cites D553, the row that removed the binary size gate, in place of D552.
