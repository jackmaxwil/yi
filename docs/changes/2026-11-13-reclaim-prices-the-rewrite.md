---
issue: Closes #2182
raise: crate runtime +79, tests +62, comments +3
decision: reclaim counts a cut's rewrite as the route's own cache write price over its read price when the catalog prices both, keeps ten reads for a route that prices cache at zero, and counts a cut's savings over at most forty requests, not the requests so far (amends D558) | in one 211-turn Sonnet 5.5 session 34 cuts made 35 turns rewrite the whole conversation, $9.11 of $18.61, because ten reads undercounts a 25-read write and a long session made every cut easier to approve | revert the change: `rewrite_reads` returns `REWRITE_READS` and the horizon is the requests so far
---
Reclaim no longer cuts old tool results when the rewrite it forces costs more than the cut saves on this route. A cut changes the middle of the prompt, so everything after it is written to the cache again. The rule counted that rewrite as ten cache reads whatever the route charged, but Sonnet 5.5 charges 25 (write $2.50, read $0.10 per million tokens), and it counted the savings over every request sent so far, so the longer a session ran the easier a cut was to approve. In one recorded session 34 cuts made 35 requests rewrite the whole conversation, up to 195k tokens each, and held the cache rate at 84.3% against 97.2% without them.

The rewrite is now counted at the route's write price over its read price when the catalog prices both, and still at ten reads for a route that prices cache at zero. A cut's savings are counted over at most forty requests. A lapsed cache still cuts at once.
