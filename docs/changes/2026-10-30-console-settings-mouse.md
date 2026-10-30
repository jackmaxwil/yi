---
issue: Closes #1736
raise: crate console +157
raise: tests +291
raise: comments +12
growth: +157 the pane's own hit table, the one cursor window shared with the sidebar, and the wheel; weighed folding the second row walk into `targets` (done, −9 lines) before adding anything
---

The console's settings page answered the pointer on its tab strip and nothing
else: a press on a row fell through to the pane body, began a text selection and
left the control alone, so every value on every tab needed the keyboard. Each
drawn row is its own click target now. A press focuses the row and works the
control it names: a toggle flips and saves, a choice steps to the next value and
saves, a number, text or list row opens the comma-or-number editor with the
cursor in it, and a row that is only editable in `~/.yi/config.json` says so in
the status line. The hover that lifts a tab or a button lifts a row too, and the
row geometry is one function the drawing and the hit table both read, so a
pressed row can never be a row the frame did not draw. A wheel over the pane
moves the cursor, so the rows a short pane keeps past its fold are reachable by
pointer too, and a tab target now stops at the pane's own edge instead of
reaching the split. The way out was already the pointer's: the gear and the
pane's close mark both close the pane.

The keyboard keeps its own route untouched — Enter still opens a choice's
picker, `u` still unsets, `d` still makes a session option the default — so the
arrows and the mouse write the same request.
