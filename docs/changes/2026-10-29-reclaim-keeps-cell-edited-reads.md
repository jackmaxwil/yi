---
issue: Refs #1242
decision: the edit side is the `[path#TAG]` headers a direct `edit` or `write` prints plus the `ok` `edit` and `write` rows on a cell's `ipython` result, matched by path suffix as before (revises D398) | a program's edit reached the session only in its cell's `calls` rows, so the read whose tags the next edit needs was cut once it aged | delete the `ipython` arm in `reclaim::newest_reads`
raise: crate context +11, crate runtime +58, tests +48, comments +1
---
Reclaim no longer cuts the newest read of a file that a kernel cell edited through `yi.tools.edit` or `yi.tools.write` (Refs #1242). Before this, only the model's own `edit` and `write` results marked a file as edited. A program's edit appeared only in its cell's `ipython` result, in the `calls` rows, so the read whose line tags the next edit needs was cut once it aged, like any other read. Reclaim now reads those rows the same way compaction's modified files and the todo landed count do. Rows for edits that failed do not protect a read.
