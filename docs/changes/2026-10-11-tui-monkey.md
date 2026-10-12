---
issue: Closes #1268
raise: tests +49
---
`tui_see.py monkey` plays seeded random key runs through the real headless solo and console loops and reports only the runs that crash, hang or leave a control character in a cell (Closes #1268). A hand-rolled splitmix64 turns a seed into a drive script biased toward Yi's real bindings: the solo keymap, the console's alt, ⌘ and ctrl+b chords, mouse, typed text and slash commands, with a quarter of the scripts pasting control characters no key produces. The script and its stderr are kept under `target/see/monkey/<seed>/`, and the report prints the `tui_see.py run` line that replays it. A fixed-seed tier-2 journey plays four seeds under `just journeys`.
