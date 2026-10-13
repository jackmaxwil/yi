---
issue: Closes #1236
raise: crate context +4, comments +1, tests +54
decision: yi-context depends on yi-tools, so compaction reads an `edit` call's files from the `[path#TAG]` section headers of its `patch` with the hashline patch parser rather than a second header parser | the `edit` tool takes only `patch`, so compaction's file tracking, which read a `path` argument, recorded no edited file in production: `modifiedFiles` held only `write` paths, the brief named an edit with no file and no read was ever marked stale; yi-tools does not depend on yi-context, and yi-runtime, the only consumer of yi-context, already builds yi-tools | drop yi-tools from yi-context's manifest and boundaries row and move the header parser into yi-context
---
Compaction now records the files an `edit` changed. The edit tool carries its paths in the patch's `[path#TAG]` headers, not in a `path` argument, so every edit was skipped: a compaction's modified-files list named only files the `write` tool created, the brief showed a bare `edit`, and a read of a file edited later was never marked stale. Both now come from the patch headers, one entry per file for a multi-file patch.
