---
version: 0.956.0
issue: Closes #2014
---
`just push` now installs `x86_64-unknown-linux-gnu` when rustup lacks it, then runs clippy for the branch's changed crates with all targets and warnings as errors, plus the `yi-cli` no-default-features configuration. A diff without crate paths lints the workspace. A machine without rustup or cargo, or one whose target install fails, prints a named skip and can still push.

The postmerge recorder on main now runs the existing dist-size ratchet after recording change files. A Linux binary measured beyond its stored budget gets its own `Ratchet: linux dist X -> Y, as the forge measured it` commit before main is pushed, so later branches do not inherit that size drift. The record job uploads a bundle of the new commits. A separate push job downloads that artifact without checking out landed code and pushes from a fresh clone with hooks, filesystem monitoring and inherited Git configuration disabled, so build code cannot reach the main-writing token or control its push. A failed ratchet still lets the recording commit push, then fails the push job.
