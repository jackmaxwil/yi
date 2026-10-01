---
issue: Closes #953
---
A plan check gets the same 600 s whichever way it is written (Closes #953). A check written as `decider: {cmd: "..."}` froze a 60 s deadline while a delegation's accept command froze 600 s (`goal::DEFAULT_CHECK_TIMEOUT_MS`), and `done` runs it in a fresh copy of the tree, so a cargo check pays a cold build: a yi-runtime suite that passed by hand came back `checker timed out (deadline 60000 ms)`. Every check now defaults to 600 s; a cold `cargo test -p yi-runtime --test integration --no-run` in a fresh copy measured 54 s. A warm per-project cargo cache was built and measured (44 to 54 s warm, the cache growing 150 to 570 MB a run) and dropped.
