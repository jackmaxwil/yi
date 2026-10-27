---
issue: Refs #1678
---

The dist binary no longer embeds the checkout path. The kernel's last-resort python root was the compile-time manifest directory; it is now `/nonexistent/yi/python`, which fails closed instead of loading code from the cwd. The binary-size gate fails when `target/dist/yi` contains the checkout, cargo registry or rustup toolchain path (also resolved), so a path that leaks back in is red on its own reason. The dist build and the gate derive those cargo and rustup roots from one helper. Two offline dist builds of this tree from checkout paths of different lengths both measured 8,686,704 bytes, so checkout path length did not move the measured size; this does not explain the original three-worktree spread or claim byte-identical binaries.
