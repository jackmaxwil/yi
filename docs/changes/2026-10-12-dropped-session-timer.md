---
issue: Closes #1322
raise: tests +52, comments +6, crate runtime +53
---
A dropped session no longer keeps its heartbeat timer running (Closes #1322). The kernel's host registry holds the session's `HeartbeatService`, and the service's lane holds the session itself, so the service was never dropped and never withdrew its lane: the timer interned for `<sessions>/schedules/<id>/scheduled-jobs.json` kept claiming, delivering into the dead session, and writing its stale in-memory ledger over whatever the session's next holder had written. `AgentSession` now withdraws its heartbeat lane when it drops, which releases the interned timer, so the next bind of that path opens the file afresh. A new test drops a session, pauses its clock through a second store on the same path, and checks the pause is still on disk after the dropped session's tick was due; the restart test no longer needs a second spelling of the sessions directory to get a fresh timer.
