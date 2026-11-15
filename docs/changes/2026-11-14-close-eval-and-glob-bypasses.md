---
issue: Closes #2175
raise: crate permission +9, tests +8
---
The shell permission gate now treats `eval` as a wrapper, so `eval rm -rf .git` and `eval cd crates && echo hi > evals/x` are judged as the command they run. A word with a glob character is judged twice, by its literal spelling and by its directory prefix, so `cat 'key-copy[1]'` reaches a hard-linked credential file whose name contains brackets. The wrapper and redirect tests cover `eval`, `strace`, `exec` and `builtin`.
