---
issue: #2163
growth: +291 one result painter shared by tool cards, the solo notebook and the console notebook; weighed keeping the old plain-when-unsure body in cell.rs (62 lines deleted for it) and a hand-rolled painter per surface, which is three copies of the escape decoder and path scanner
decision: tool results are coloured from a fresh parse at every break in the shown text (the first row, a line-number gap, an elision row, a clipped row, a parse error) instead of left plain when the parse cannot be sure | most reads start mid-file and every search hit is lifted out of its file, so the plain rule left nearly every result uncoloured | reversible by restoring the line-1 check in the tool card body
cut: crates/tui/src/cell.rs | if number.parse::<u64>().ok() != Some(expect) { | colour restarts at a line-number gap instead of stopping; the decision line above revises the rule
cut: none crates/tui/src/pycell.rs
cut: crates/tui/tests/highlight.rs | fn a_grep_hit_is_left_plain() -> TestResult { | a search hit is now coloured by its file's language; its test asserts the restart rule instead
ledger+: TUI | every tool result is coloured: a read or edit by its file's language from any line it starts on, search hits and their context rows by each file's language under the file's path, a streaming write by its path and a streaming cell as Python, other output by what it holds (JSON, a diff, a shebang), the colour codes a program printed as colour, and file paths, `path:line` included, in the hue prose gives a path; the console and solo notebooks paint cell output the same way
raise: crate console +1, crate tui +290, tests +365, comments +2
---
Tool results are coloured wherever they appear. A read that starts in the middle of a file, the
tail after a long result's `… N more lines` row, and every search hit are coloured by the file's
language. A search groups its hits and context rows under the file's path and drops the anchor
tag. Shell and notebook output shows the colours the program printed instead of `␛[31m`, and any
other escape sequence is dropped. JSON, diffs and scripts with a shebang are typed by their
content, and file paths, `path:line:col` included, take the theme's status-path colour.

The parse restarts from the grammar's first state at every break in the shown text, so a row
that opens inside a string or a comment colours wrong at most until the next break. Painted card
that opens inside a string or a comment colours wrong at most until the next break. Every
surface paints a body as it draws it, with no cached copy: a memo keyed on the card's own text
cost more to rebuild and hand back than the paint it saved, and the console never reached it.
