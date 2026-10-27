---
issue: Refs #1678
---

The dist binary no longer embeds the checkout path. The kernel's last-resort python root was the compile-time manifest directory, so the same commit measured 8,653,648, 8,670,192 and 8,703,216 bytes in three worktrees; it is now `python` relative to the cwd. The binary-size gate fails when `target/dist/yi` contains the checkout path, so a path that leaks back in is red on its own reason.
