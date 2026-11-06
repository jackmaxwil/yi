---
issue: Closes #2014
---
`just push` now installs `x86_64-unknown-linux-gnu` when rustup lacks it, then runs clippy for the branch's changed crates with all targets and warnings as errors, plus the `yi-cli` no-default-features configuration. A diff without crate paths lints the workspace. A machine without rustup or cargo prints a named skip and can still push.

The postmerge recorder on main now runs the existing dist-size ratchet after recording change files. A Linux binary measured beyond its stored budget gets its own `Ratchet: linux dist X -> Y, as the forge measured it` commit before main is pushed, so later branches do not inherit that size drift. The checkout does not persist `RECORDER_TOKEN`; only the push step receives it. A failed ratchet still lets the recording commit push, then fails the job.
