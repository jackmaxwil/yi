---
issue: Closes #965
raise: crate tools +12, tests +17, comments +1
---
The turn checkpointer no longer stages its own shadow into itself when the project contains it (Closes #965). With `yi` started in the home directory, or a test rig whose home sits under its cwd, every capture added the previous capture's loose objects as new blobs: eight captures took the shadow from 28 to 3,337 objects, one taking 16 s, and `plan_trace` ran past nextest's 60 s limit. `Checkpoints::open` compares canonicalized paths and, when the checkpoint root lies inside the project, every `add --all` (one `stage` helper for `capture`, `capture_excluding`, `changed` and `restore`) carries a `:(exclude,literal)<root>` pathspec.
