---
issue: Closes #1758
raise: crate context +1, tests +97
---
The compaction retention floor now keeps only the words the user typed (and a child's task request, which the model also reads bare). Host notices, job and mail announcements, `<yi_internal_context>` wrapped reminders and earlier compaction summaries are no longer carried into the kept tail as if the user had said them; the summary covers them. `retain_floor` asks `Attribution::reads_as_typed`, the same test the request converter uses, so a message stored before attribution existed still counts as typed. The stored session is unchanged: a cut is a view over the ledger.
