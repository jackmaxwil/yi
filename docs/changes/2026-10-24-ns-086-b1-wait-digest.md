---
issue: Closes #1519, Refs #1442
growth: +7 one roster fold that counts each state and keeps only the movers' details, with the existing roster retained for a settled reply's finished list; no second roster read or new helper
raise: crate runtime +7, tests +103
decision: rlm.wait reports counts by state and the states of the members that moved; rlm.status holds the roster | a 128-wide family pays the whole roster on every wait | restore the full states map in wait_reply
---

B1-wait-digest: `rlm.wait` now returns `counts` by state and limits `states` and `notes` to the members named by `causes`. A settled reply still lists every finished child in `finished`, and `rlm.status` remains the full roster. The delegation prompt teaches that split.

The pre-registered zero-spend gate measured a 9,325 B reply from 128 runtime-built faux readers with one mover, above the 4,096 B build threshold. The same reply is 4,144 B after the change. Tests cover that wide reply and a three-reader settled family.
