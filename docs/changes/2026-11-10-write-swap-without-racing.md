---
cut: none crates/runtime/tests/permission_scope.rs
---
The two write race tests are gone, and the four check-then-open race tests no longer time out on
a Mac. Each write race test needed 5,000 swaps landing inside the window, and every write's
`sync_all` is a full drive flush on macOS, about 5 ms. Alone on a quiet machine the link swap took
134 s and the parent swap 61 s, past nextest's 60 s kill, and beside them the two read race tests
stalled on the same flushes and timed out too. Linux CI flushes cheaply, so only Macs saw it.

The write tool's gate is now proven the way edit's and grep's are: one call per open, with the
parent swapped to a link to the key store at each open, in 0.06 s. A write that opens by path
instead of through the gate fails it. What it does not catch, and the race tests did, is a write
that reopens by path after the gated open; the read race tests still cover that shape for read.
For write, a name swapped after the open is refused by the staged replace's own identity check,
which a unit test in crates/tools/src/tool.rs covers for a link swapped in before the rename.
