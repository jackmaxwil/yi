---
issue: Closes #1179
raise: crate cli +1, crate runtime +126, comments +4, tests +9
---
The line selector now sees and adds only the lines a cut left out (Closes #1179). The cut result's lines are matched in order against the full output, jumping past each `[N … omitted: A-B]` row the reducer writes, so the selector is sent only the hidden lines, numbered as in the full output, and a line it names that the result already shows is dropped and counted in the row. A cut that hid nothing asks nothing. Dogfooding found 16 of 17 selected lines duplicating the kept tail of a verbose pytest run, and the reducer's own ERROR line sent again on a 3,000-line log.
