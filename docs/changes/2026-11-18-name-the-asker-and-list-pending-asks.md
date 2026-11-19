---
issue: Closes #2261, Closes #2263
cut: crates/acp/src/daemon.rs | flush_parked | moved to daemon/pending.rs, which also sends the held requests a client lacks; the worker_requests comment went with the insert into hold_request
shares: #2300 both edit acp_unit.rs and runtime/src/lib.rs: this branch adds one test and no change to lib.rs beyond the stacked #2298 re-export
growth: +298 the daemon's held-request table with its listing and resend, the asker meta, and the wire types; nothing deletable, the daemon kept only an id-to-root map
---
A child agent's permission request named no child, and a request already delivered to a client that then dropped was never sent again. `session/request_permission` now carries `_meta.yi.asker` with the child's id (absent for the root session). The daemon keeps each unanswered request, sends it again once to a client that attaches after the first delivery, and answers the new `_yi/pending` with requests still open in the client's attached sessions, optionally for one session. A stopped ask tells the daemon through `_yi/permission_closed` so the request leaves the list.
