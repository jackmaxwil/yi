---
issue: Closes #1519, Refs #1442
growth: +53 the digest adds a settled-family count path and builds full views only for movers; no full roster is retained in a settled reply
raise: crate runtime +53, tests +85
decision: rlm.wait reports counts by state and the states of the members that moved; rlm.status holds the roster | a 128-wide family pays the whole roster on every wait | restore the full states map in wait_reply
---

B1-wait-digest: `rlm.wait` now returns `counts` by state and limits `states`, `notes` and a settled reply's `finished` list to the members named by `causes`; `rlm.status` remains the full roster. A settled family counts terminal records without reading every child's session and builds full views only for movers. Python result waiters share one digest resolver, which checks registration independently of the bounded reap log, and the scheduler shares one digest merge. The delegation prompt teaches the split.

The pre-registered zero-spend gate measured a 9,325 B reply from 128 runtime-built faux readers with one mover, above the 4,096 B build threshold. The same reply is 334 B after the change, with the threshold asserted in the test. Tests cover that wide reply, a three-reader settled family, a reap missing from the digest, and sibling-only digests through all three Python consumers.
