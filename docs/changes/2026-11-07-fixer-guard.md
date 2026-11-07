---
issue: Closes #2075
---
A fixer's push is refused when it undoes what the pull request's own commits did on purpose. `scripts/fixer_guard.py` reads the commits a push adds against the PR's earlier commits: re-adding a line an earlier commit deleted, or deleting five or more substantive lines one added, refuses the push; a review finding that asked for it is shown beside the hit and exempts nothing, labels the PR `autofix:hold` and comments each hit. The autofixer runs it before every push; the land-fix script calls `fixer_guard.py --pr N --from <head before the fix>` before its own. Its selfcheck replays the two incidents that made it, #2062's README and #1829's upstream pin.
