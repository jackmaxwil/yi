---
issue: Closes #2172
---
A review round with a medium finding that holds the draft is now `blocked`, not `clean`. #2165 merged
with five open medium findings, one of them a broken sentence that went into the changelog: the round
read `clean` because only a high finding blocked, so `just pr ready` and the merge queue, which refuse
only a blocked round, let it through, and the fixer, which answers drafts, never saw the mediums. A
medium holds a round through round 2, and from round 3 on only while it is carried from an earlier
round, as the draft rule already said. A fresh medium from round 3 on stays advice. The fixer answers
a blocked round that has only mediums, and an `/override` still clears the round.
