---
issue: Closes #1520
growth: +316 includes #1945's lines beneath it in this stack; the door (resolve by path, the config every host git takes), the guard on a planted HEAD, the identity fallback and trunk. Weighed: a memo of resolved paths, cut since a walk costs stats beside a spawn; four separate runners kept, which is the defect; hostgit.rs is mostly a move out of lane/mod.rs, past its size ceiling
raise: tests +235, comments +20, crate runtime +163, crate tools +89
decision: every git the host runs goes through one door, by absolute path, with hooks, fsmonitor and signing off | a host hand-back ran the repository's hooks, a relative core.hooksPath is written by the child, and a tree's bin on PATH ran a contained write on the host | restore capture_named and git_capture to bare names
---
Yi's own git calls now go through `yi_tools::host_command`, and every other program it runs on the host resolves the same way: to an absolute path outside the trees a contained process can write. Before this, a tree's `bin` on `PATH`, as direnv or an activated venv leaves it, ran a contained write on the host's next `git status`.

A hand-back now commits past the repository's hooks and signing, with a fallback identity, so a red `pre-commit` no longer strands a child's work; a red surfaces at `check=`, the next `just commit` or the forge. Untracked `target/`, `node_modules/`, `.venv/`, top-level `dist/` and `build/` stay out of a candidate. A `HEAD` planted on trunk or on a branch checked out elsewhere is refused rather than committed to.

Plumbing a caller parses is read whole or fails naming the cut: a long `git status -z` lost its middle, and a dirty file with it. The lane code now reads trunk from origin's `HEAD` instead of assuming `main`.
