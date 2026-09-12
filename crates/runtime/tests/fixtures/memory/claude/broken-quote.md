---
name: broken-quote
description: "The forge gate is the only lane since #308; there is no local pre-push lane
metadata:
  type: project
---

`just prepush` still runs by hand and covers macOS; the pre-push hook no longer
calls it.

**Why:** PR #308 moved the lanes to the forge gate.

**How to apply:** push, then read the verdict with `just pr status N`.
