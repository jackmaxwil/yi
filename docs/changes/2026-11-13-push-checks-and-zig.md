---
version: 0.1042.0
issue: Closes #2186
cut: none scripts/forge_pr.py
shares: #2167 it edits cmd_commit and linux_clippy for the daemon resume work, in hunks next to the ones here, so whichever lands second merges main and keeps both
---
`just push` on a Mac now runs the Linux clippy step with the repo's zig wrappers as the C compiler and archiver when `zig` is installed, so `ring`'s C build finds a sysroot. Before, the step failed with `fatal error: 'assert.h' file not found` unless four variables were exported by hand. Without zig, a failed step prints the install command.

`just push` also runs the net-deletion check that `just pr open` ran, so a branch that deletes lines with no `cut:` line is refused before the multi-minute push lane, not after it. `just pr open --help` says `--body` takes a file path.
