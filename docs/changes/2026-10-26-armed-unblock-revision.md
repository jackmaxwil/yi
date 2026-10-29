---
version: 0.770.0
issue: Refs #1442
raise: crate runtime +38, tests +191, comments +4
---
An armed plan wait was checked and then unblocked without carrying the checked plan revision into the operation. Two firings could both read the row as blocked; the first moved it to pending, and the second failed with an illegal unblock instead of holding. The same gap could let a stale firing unblock a newer wait.

The unblock now binds to the revision returned by that read. A stale revision rereads the wait condition and retries, while a row that no longer waits on the address is reported as held. Competing firings therefore unblock the row once rather than racing into an illegal step or unblocking a different wait.
