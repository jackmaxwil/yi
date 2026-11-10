---
decision: Push this user-authorized branch to the GitHub mirror without a tracker issue | the private Forgejo endpoint and `fgj` are unavailable and GitHub issues are disabled; the owner said “Push to remote. We override.” | return future changes to the Forgejo issue gate once access is restored
growth: +312 focus-aware title pulses and OSC output; weighed removing the title wake scheduler and OSC assertions, but that leaves titles stale or untested
raise: tests +215, comments +2, crate console +226, crate tui +86
ledger=: terminal tab run state | 17.3, 17.4 | gated tui (default feature) | M | M | the focused foreground run title pulses `◌ ◍ ◉ ◍` while working, shows `!` when it needs the user, and has no marker when ready; full Kitty orb stays in terminal content
---
Yi's solo TUI and console titles keep their session labels and reflect the focused foreground run. Working uses the four-frame text pulse every 250 ms; attention uses `!`; ready titles omit the marker. Background jobs and child sessions may outlive the parent run without keeping its title pulsing. The full Kitty orb remains in terminal content. Ghostty's tab title is text-only. Environment Git probes and the incident-backed spill-scale timing test remain in `just test` and run after Nextest drains its parallel pool, preserving the 200 ms bound.
