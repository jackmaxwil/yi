---
issue: Closes #990, Refs #991
raise: tests +3, crate runtime +5
---
A reader or worker spawned with a `family://` partition is refused at spawn with a line naming `context_keys` (Closes #990). A recipe that `rlm.put` a value and gave a reader `family://<name>` left the reader only the dill sidecar, since only a member with a kernel can `rlm.get` it. The orchestrate doctrine and the `rlm.put` docstring now say a kernel-less child reads a computed value through `context_keys`.
