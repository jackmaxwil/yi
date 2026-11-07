---
name: linker-error-first-symbol
description: For a linker error read the first undefined symbol the log names, not its last line.
type: feedback
---
**Why:** the last line of a macOS linker failure is the framework list, and it names nothing that is missing.

**How to apply:** grep the log for `Undefined symbols` and read the first `_name` under it, then the crate that should export it.
