---
issue: Closes #971
raise: crate runtime +101, crate tools +10, tests +486, over-cap +1, comments +17
---
A walled session walls every session store, `~/.yi/sessions` and the `--session-dir` in use, and can read only its own transcript file (Closes #971). The stores join the walled session's `deny_read` beside the spill roots, so the read gate and every walk refuse them; a contained command's Seatbelt profile denies each store and re-allows the own transcript; `local://`, `checkpoint://`, `tree://` and `plan://` judge by the same wall; a walled holder's broker refuses an approved retry whose remembered refusal lies in a protected dir; a refusal under the session's own walls is no longer remembered; and a child broker keeps the `--session-dir` as host-owned. Found in the review of #968: a juror-shaped walled session could read the author's transcript, which holds everything the wall hides.
