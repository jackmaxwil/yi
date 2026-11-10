---
version: 0.1020.0
decisions: D578, D579
issue: Refs #2129
decision: The improve gate's economy road reads cost, then wall with cost inside ten percent, never raw tokens on a priced route; cost is provider dollars where every trial is priced and tokens (cache reads and writes counted) where a route bills $0 (amends D220) | the owner's objective is dollars per graded point, wall second, and D558 showed a $0 route hides its spend from a price test | put tokens back among levers.judge's road keys
decision: The eval cost counts side:* and tool:* usage records beside assistant usage | side calls were booked in the session file but no eval read them, so a side model's cost was invisible to the gate | drop the record branch in extract.py's cost sum
---
The session-mining extractor counts a harness census per session: requests, actions (tool results plus nested kernel calls), resolved re-check turns (a command-only turn that re-runs a check right after a clean edit), stale runs (calls that ran after a failed write or command in their own message), writes after a failure, spill reads with and without the line selector, and the subjects each doctrine section speaks to. `--sessions` takes several directories, and the report prints the census and a doctrine-relevance table by task family, counts only.

`levers.py judge` takes two trial files and prints one verdict with its unit, its development spend and the tasks it needs to break even; `--store` appends the verdict row the improve round's S0 reads, so a rejected variant is no longer re-run. The economy road now passes on cost, then on wall while cost holds, and a route that bills nothing is judged by its tokens. A proposer's spend is recorded in its `s0.json`. `axes.py` prints an `a/r` line for the ledger's new rightmost column.
