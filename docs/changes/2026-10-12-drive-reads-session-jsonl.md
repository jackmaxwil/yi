---
issue: Closes #1362
raise: tests +3
---
The control-socket drive test's stored-session helper now reads only session JSONL files and passes listing and read errors through (Closes #1362). Every root session writes `<sessions>/schedules/<session id>/scheduled-jobs.json` beside its JSONL, because its mailbox job is stored there at bind, and the helper had been reading that directory as a file. The earlier repair skipped whatever failed to read, so a real I/O error would have surfaced as an unrelated assertion.
