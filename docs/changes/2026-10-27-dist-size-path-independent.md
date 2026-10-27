---
issue: Refs #1678
---

The dist binary no longer embeds the checkout path. The kernel's last-resort python root was the compile-time manifest directory; it is now `/nonexistent/yi/python`, which fails closed instead of loading code from the cwd. The binary-size gate fails when `target/dist/yi` contains the checkout, cargo registry or rustup toolchain path (also resolved), so a path that leaks back in is red on its own reason. This removes the one embedded path found with `strings`; the cause of the reported spread between worktrees is not confirmed, so the budget above it carries an unverified premise until evidence lands: dist builds of one commit in two worktrees whose checkout paths differ in length, compared byte for byte, plus a `strings` scan of the binary for any absolute path outside the three roots the leak gate scans. Until that evidence exists, a size failure outside the scanned paths is a finding, not a ratchet case.
