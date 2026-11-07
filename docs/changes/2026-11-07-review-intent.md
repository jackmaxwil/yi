---
issue: Closes #2084
---
The review bot reads recorded intent. Each lens is shown the `cut:` and `keep:` lines that touch the PR's files, from the PR's own change file and main's. A finding that would put back a cut line (it sits within three lines of where the PR deleted it) or delete a kept line (its quote is that line) is dropped before any refuter. The round lists each one with the change file that recorded it. On #2062, round 1's high asking to restore the owner's cut install block would have been dropped once the cut was recorded.
