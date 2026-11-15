---
issue: Closes #2253
cut: none crates/runtime/src/fetch/mod.rs
growth: +0 two retired-state reads in the family live count, one test
raise: crate runtime +35, tests +21
---
A root that had spawned and deleted fifteen children was refused with "the family holds 16 live sessions" while `rlm.status()` listed none. The family count kept any session something still held a reference to, so a deleted child kept its seat. The count now skips a session once it is retired and a kernel once it is disposed, whatever still references it.
