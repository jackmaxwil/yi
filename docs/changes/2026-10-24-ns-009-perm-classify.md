---
issue: Closes #1451, Refs #1442
raise: comments +2, crate permission +10, tests +41
---
Yolo's destroy belt now reads every segment of a command. It used to read only the first word of the whole string, so `cd build && rm -rf ~/.ssh`, `true; rm -rf .git` and `echo done && rm -rf ~` ran in yolo although the yolo prompt promises catastrophic targets are always denied. The belt now splits where a shell may start another command (`;`, `&`, `|`, a newline, a parenthesis, a backtick) and judges each segment that runs `rm`, `rmdir`, `shred` or `unlink`. It splits the raw text rather than taking the classifier's segments, because the lenient splitter strips a leading `$` and would stop `$HOME` from expanding to the home directory. On macOS the destroy gate also compares paths with letter case folded, since a default volume opens `~/.SSH` as `~/.ssh`; folding both sides only ever denies more. Member unit: HA-PERM-belt.
