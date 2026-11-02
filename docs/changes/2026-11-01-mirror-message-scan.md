---
issue: Closes #1984
---
A commit message that the public mirror would refuse now fails the branch's guardrails instead of stopping the mirror after the merge. `check_public_surface.py` reads the messages on `HEAD --not` the branch's fork, applies `replace.txt` as the mirror does, and matches them against `deny.txt`. A hit names the commit, and the refusal says to reword it or add a literal. Author and committer identities are still judged only by the mirror job, since this does not model `mailmap.txt`.
