---
issue: Closes #1020
raise: crate acp +3, crate runtime +4, tests +26, comments +2
---
Closing an ACP session stops its work (Closes #1020). `session/close` answered every prompt still queued in the ACP layer as cancelled, but the session's turn ran on, and a prompt already handed to the session as a follow-up ran after the close and landed in the transcript. A close now cancels the session, so no further run starts, and aborts the turn in flight.
