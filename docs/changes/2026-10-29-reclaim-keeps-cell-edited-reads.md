---
issue: Refs #1242
raise: crate runtime +57, tests +48
---
Reclaim no longer cuts the newest read of a file that a kernel cell edited through `yi.tools.edit` or `yi.tools.write` (Refs #1242). Before this, only the model's own `edit` and `write` results marked a file as edited. A program's edit appeared only in its cell's `ipython` result, in the `calls` rows, so the read whose line tags the next edit needs was cut once it aged, like any other read. Reclaim now reads those rows the same way compaction's modified files and the todo landed count do. Rows for edits that failed do not protect a read.
