---
issue: Closes #1364
raise: crate session +16, comments +2
---
A loaded session now holds each entry and lane record once. `SessionState` kept a full clone of every entry in `entries` beside the same entry inside its append-only mutation `log`, so a session's resident size was about twice its parsed content; `entries`, `records` and `entries_by_id` are now positions into the log, and `SessionStore::grep_page` walks the entries by reference instead of cloning every one per search. Resuming a real 4.8 MB session with `yi sessions show` peaked at 49.6 MB resident against 64.8 MB before (debug build, best of three) (Closes #1364).
