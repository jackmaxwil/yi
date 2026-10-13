---
issue: Closes #1371
growth: +186 the pointer chrome and hover plumbing consolidated into one hit-test resolver, a focused guard on `orb::tick` so unfocused panes hold one placed frame instead of animating, and the same focus test in `chat_wake` so they stop scheduling wakes; per-widget hit fns and a second settings-strip painter were weighed and rejected in favor of reusing `strip()` and `paint_rgba`
decision: a lane session's status row names the repo alone: `LaneHandle::row` stays the one builder and `_yi/workdir` still carries its text, but the `⎇ lane <slot>` suffix is gone (amends D208) | the owner asked for no "⎇ lane 0" on the console status row (#1371); the slot number is pool bookkeeping nobody acts on, and the row already says which tree by its repo | restore `format!("{repo} ⎇ lane {}", lane.slot())` in `LaneHandle::row`
raise: crate console +182, crate tui +6, tests +203, comments +5
---
The console's chrome answers the pointer (Closes #1371). A click on a settings tab opens that
tab; the strip records its targets from the same pieces it draws, so a click can never miss a
name. Hovering a sidebar session, the gear, a pane title, a pane's close mark or a settings tab
lifts it a shade, and a reversed chip lightens its face instead. One hit test decides what is
under the pointer, and a move redraws only when that changes. A pane with no session titles
itself without the grey status dot, so the settings pane reads `Settings`. Every chat pane on
screen keeps its Yi mark when focus moves to another pane or to the sidebar; only a popup hides
it, and the model logos, whose image ids are fixed, stay with the focused pane. The sidebar
drops one blank column between the avatar and the text, in the rail (8 columns wide now, 9
before) and in the full list. The lane label on the status row reads the repo name, `yi`,
instead of `yi ⎇ lane 0`. An unfocused pane also stops waking the console loop for its orb, so
a pane with a running agent no longer spins the loop at about 1000 wakeups a second. The
console work grew the dist binary from 8,372,272 to 8,388,784 bytes, 176 bytes past the old
8 MiB cap, which the 9 MiB cap in #1380 absorbs.
