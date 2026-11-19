---
issue: Closes #2261, Closes #2263
raise: comments +4, crate acp +177, crate runtime +28, crate types +40, tests +118
growth: +298 the daemon's held-request table with its listing and resend, the asker meta, and the wire types; nothing deletable, the daemon kept only an id-to-root map
---
A child agent's permission request named no child, and a request already delivered to a client that then dropped was never sent again. `session/request_permission` now carries `_meta.yi.asker` with the child's id and name (absent for the root session). The daemon keeps each unanswered request, sends it again once to a client that attaches after the first delivery, and answers the new `_yi/pending` with every request still open, optionally for one session. A stopped ask tells the daemon through `_yi/permission_closed`, so a stopped ask is neither listed nor sent again.
