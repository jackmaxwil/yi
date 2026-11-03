---
version: 0.900.0
issue: Closes #1544
raise: crate context +9, tests +136
---
The retention floor keeps only the requester's words and the letter that wakes a session, and one lifetime table in `wrapper.rs` retires a host notice with the window that read it (Closes #1544). `retain_floor` kept every user-role message in the summarized range, so a `Host(Notice)` deadline or restore note that the summary already carried rode the retained tail for the rest of the session, and `drop_internal` knew nine wrapped kinds and five read-once nudges: `stream_retry`, `todo_prelude`, `cache_alert` and `plan_relevance` accumulated in every later window. A child's brief is `Task` and the floor still keeps it, so no compaction takes a child's task from it (D337).
