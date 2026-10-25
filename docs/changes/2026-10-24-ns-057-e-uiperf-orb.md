---
issue: Closes #1493, Refs #1442
raise: tests +148, comments +4
---
Every orb on the screen used to solve its own pairings. A working session's avatar and the chat orb each held a private table of finished morphs, so eight avatars changing state together started eight threads, each building a 540 by 540 cost matrix for the same pair of poses. At the optimized profile that startup spends about 34 ms of CPU per solve, 1,099 ms for eight avatars through four state changes, and 4 solves is all it needs. The finished pairings now live in one table for the process, keyed by the state left, the exit phase and the state entered. A key that is still being solved is joined rather than solved again: every avatar that wants it polls the same cell and keeps holding the exit pose until the answer lands. The table lock is taken once when a state change starts, for a lookup or for an insert together with its thread spawn, never per frame and never while a pairing is computed.

A superseded request no longer stops its matching thread. The thread runs to the end and its result is kept, because another avatar may be waiting on that same key and an abort by one orb would have starved the rest.
