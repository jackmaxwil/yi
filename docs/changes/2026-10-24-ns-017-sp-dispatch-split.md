---
issue: Refs #937, Refs #1442
raise: crate runtime +16
---
The plan engine's spawn mapping now lives in its own module, `plan/spawn.rs`, instead of at the head of `plan/dispatch.rs`. The four functions that turn a todo's delegation into a child (the child's name, the acceptance line, the brief it is told, and the spawn kwargs the host reads) moved with the four tests that call the brief and the kwargs. `dispatch.rs` sat at the 1,200-line file ceiling, so no later change to the spawn mapping or the dispatch half could land there; it now stands at 991 lines. Nothing about the behaviour changed: the moved code and tests are line for line what they were, with the three helpers `dispatch.rs` calls made `pub(super)` so they stay inside the plan module. The crate grows by 16 lines: the new file's import header and test-module header, the `mod` line, the `use` line in `dispatch.rs`, and a signature that rustfmt now wraps.
