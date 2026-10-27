---
issue: Refs #1678
---

The dist binary no longer embeds the checkout path. The kernel's last-resort python root was the compile-time manifest directory; it is now `/nonexistent/yi/python`, which fails closed instead of loading code from the cwd. The binary-size gate fails when `target/dist/yi` contains the checkout, cargo registry or rustup toolchain path (also resolved), so a path that leaks back in is red on its own reason. This removes the one embedded path found with `strings`; the cause of the reported spread between worktrees is not confirmed.
