---
issue: Closes #2046
decision: Keep a text-only OSC 2 title for the focused foreground run | the terminal tab should distinguish working, attention and ready without sending the Kitty orb image through the title | keep the full orb in terminal content and background work from pulsing the parent title
decision: Raise the local `yi --version` startup ceiling to 6 ms | the 5 ms ceiling rejected a 5.18 ms run and matched source A/B runs showed no stable source delta | lower it after repeated measurements stay below 5 ms
growth: +353 title pulses, OSC output, raw-argument version path, dep-info and worktree metadata fixes, and 5 runtime formatting lines; weighed removing the title wake scheduler and OSC assertions, but that leaves titles stale or untested
raise: tests +215, comments +4, crate cli +14, crate console +226, crate runtime +27, crate tui +86
ledger=: terminal tab run state | 17.3, 17.4 | gated tui (default feature) | M | M | the focused foreground run title pulses `◌ ◍ ◉ ◍` while working, shows `!` when it needs the user, and has no marker when ready; full Kitty orb stays in terminal content
---
In the solo TUI and console, the terminal title keeps the session label and marks the focused foreground run: a four-frame text pulse every 250 ms while working, `!` when attention is needed, and no marker when ready. Background jobs and child sessions do not keep the parent title pulsing. The full Kitty orb remains in terminal content; OSC 2 titles are text-only. The local `yi --version` startup ceiling is 6 ms.
