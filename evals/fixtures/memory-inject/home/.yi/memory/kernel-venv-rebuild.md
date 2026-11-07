---
name: kernel-venv-rebuild
description: Any change to python/yi_runtime moves the runtime identity hash and forces a venv rebuild on next boot.
type: project
---
The bootstrap cell's ready check is pinned by crates/kernel/tests/ready_check.rs.

**How to apply:** edit the package and the check string in bootstrap.rs in one change, or the next boot hangs asking for a rebuild.
