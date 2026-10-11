---
issue: Closes #1255
---
The review bot now waits on the `tui` workflow's own run for its head, read from the forge's run list by workflow file and commit, a queued run included. The fixer's answer to #1213's round 5 asked whether any workflow had a task for the head. The review's own job always does, so on every pull request the `tui` workflow skips, a round waited out the full 30-minute snapshot wait. The `tui` workflow also runs now when crates/types or Cargo.lock changes, since wire shapes and dependency bumps change what the TUI draws. crates/runtime and python/ stay out, because a Ghostty run on every runtime change would hold the runners the gate needs. The shared cut notice the issue asked for landed with that fixer commit.
