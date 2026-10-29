---
issue: Closes #155
raise: tests +27, comments +2, crate kernel +11
---
The kernel's namespace query answers on a second attempt instead of reporting no names. `Snapshot::list_namespace_names` is a read of a live namespace, and a listing that arrives while the kernel is busy spends its five-second budget waiting for the queue rather than listing: it came back empty, the caller read that as "this kernel has no names", and gate run #94 reddened `yi-kernel::kernel_e2e prune_removes_oversized_variables_and_list_names_reports` on a tree that touched nothing under `crates/kernel`. It now makes up to two bounded attempts, and only a kernel that misses both reports no names, with each attempt's reason in the kernel diagnostic instead of one unqualified line. The retry starts after the first attempt's abort has interrupted the stalled cell, so it measures a settled kernel. Run #95, the identical tree, was green, which is what made this a gate flake rather than a regression.
