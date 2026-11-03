---
issue: Closes #2014
---
`just push` now runs clippy for the branch's changed crates against `x86_64-unknown-linux-gnu`, including tests and warnings as errors. A machine without that target prints a named skip and can still push.

The postmerge recorder on main now runs the existing dist-size ratchet after recording change files. A Linux binary measured beyond its stored budget gets its own `Ratchet: linux dist X -> Y, as the forge measured it` commit before main is pushed, so later branches do not inherit that size drift.
