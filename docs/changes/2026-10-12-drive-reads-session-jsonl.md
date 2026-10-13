---
issue: Closes #1362
---
The cli integration tests find session files through one helper, `support/session_files.rs`, which keeps only `*.jsonl` entries under `sessions/<project>/` and passes listing and read errors through (Closes #1362). Each root session writes `<sessions>/schedules/<session id>/scheduled-jobs.json` beside its JSONL, because its mailbox job is stored there at bind. That directory broke the control-socket drive test, and the repair that followed hid every read error. Nine hand-written copies of the walk, in five test files and the prompt-slot helper, now call the one function, so the next entry added beside the sessions is filtered in one place.
