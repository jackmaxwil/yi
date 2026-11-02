---
version: 0.871.0
issue: Closes #1054
raise: tests +34
---
`grep` counts a file the read gate refuses and names it in one `[…]` row instead of answering "No matches found". A search over a key store, a device or the workspace `.git` never opened the file, so the old answer told the model a guarded store held no match when nothing had looked, which the loud-caps law forbids (045). The count and the reason are one row; nothing opens those paths, so the row names no continuation call.
