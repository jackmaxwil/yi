---
version: 0.863.0
issue: Refs #1966
---
The forge can now land reviewed PRs by itself. A tick every five minutes merges the oldest `queue`-labelled PR onto current main on `queue/candidate`, reads the gate on that commit, and fast-forwards main to it, so nobody updates a branch only to test it. It stays off until the repository variable `MERGE_QUEUE` says `shadow` or `on`. `just pr queue N` enqueues a PR, and `just pr merge --now` forces one past the queue. This takes one PR at a time; batches come next.
