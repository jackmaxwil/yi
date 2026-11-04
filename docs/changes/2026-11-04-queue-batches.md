---
issue: Closes #1966
---
The merge queue now builds each candidate from up to four of the oldest eligible PRs in enqueue order and lands a green batch with one non-force push. A red batch splits to its first half on current main instead of retrying whole, while a red singleton keeps its one retry before ejection. PRs that conflict only with an earlier batch member stay queued without a comment. The four fork-relative size checks announce a skip on the queue candidate because every member was judged at its own head.
