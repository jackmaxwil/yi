---
issue: Closes #990, Refs #991
raise: crate runtime +79, tests +28, comments +3
---
A reader or worker spawned with a `family://` partition is refused at spawn with a line naming `context_keys` (Closes #990). A recipe that `rlm.put` a value and gave a reader `family://<name>` left the reader only the dill sidecar, since only a member with a kernel can `rlm.get` it. The orchestrate doctrine and the `rlm.put` docstring now say a kernel-less child reads a computed value through `context_keys`.

A lane sync that fails (`uv sync --frozen --offline --quiet` in a sandbox that cannot write uv's cache) now refuses with the command, its own error, and, when the sandbox refused it, the sandbox's refusal hint naming the denied path, not the 17 KB Seatbelt profile before it (Refs #991, message only).
