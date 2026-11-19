---
issue: Closes #2280, Closes #2291
raise: comments +3, crate acp +7, crate runtime +45, crate types +2, tests +106
growth: +54 a stop check on the ask, polled by the ACP asker; nothing deletable, the ask had no way to hear an abort
---
A permission ask waited on a client's answer and watched nothing else, so `session/cancel` and `_yi/child_abort` left the turn, or the child, blocked until something answered. Every session now hands its broker a stop check that reads its abort signal. The ACP asker polls it every 100 ms, drops the pending request and rejects, and the journal records the reject as `stopped`, not as a person's deny.
