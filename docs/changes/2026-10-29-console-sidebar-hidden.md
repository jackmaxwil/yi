---
issue: Closes #1713
raise: tests +167, comments +5, crate cli +2, crate console +102, crate types +9
---
The console's sidebar had two states, the full list and a narrow avatar rail, and no way to
get it out of the way on a small terminal. ⌥b and ⌘B now walk three: rail, full, hidden. A
hidden sidebar is worth no columns at all, so the panes take them; it draws no rows, so
focus cannot sit in it and tab never lands there, and the header keeps its way back as
`⌥b sidebar` at the left, where the tabs start after it. The state a press lands on is
written to `console.sidebar` in config.json through `_yi/set_default`, so the next console
starts in it; the settings pane lists the same leaf, restarts-only, beside the title
options. The palette and the keys sheet carry the one chord, so the third state needs no
second binding.
