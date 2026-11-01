---
version: 0.849.0
issue: Closes #1967
---
A review round no longer reads a head again when no fix has touched it. One refuter seat that did not answer used to mark the whole round unread, so every lens read the head afresh and the next round re-read the whole PR; that now re-reads nothing, and the round's findings stand on the votes that came. A lost lens reads the head again alone. Merging the base re-reads nothing unless a line the PR adds or removes changed, where before a base edit beside one of its lines counted as new code. The reason a call went unanswered is now in the job log.
