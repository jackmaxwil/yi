---
name: ratchet-own-commit
description: A guardrail baseline moves in its own Ratchet commit, never beside the code it unblocks.
type: project
---
A raise lands before the code commit; a shrink lands after it.

**Why:** every PR that mixed a baseline edit with code was refused, so 9 of 9 conflicted.

**How to apply:** `just ratchet` makes that commit; `just commit` refuses to bury one.
