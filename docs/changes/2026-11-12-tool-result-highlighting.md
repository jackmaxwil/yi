---
issue: #2163
growth: +390 one result painter shared by tool cards, the solo notebook and the console notebook; weighed keeping the old plain-when-unsure body in cell.rs (62 lines deleted for it) and a hand-rolled painter per surface, which is three copies of the escape decoder and path scanner
raise: tests +333
raise: crate tui +383
raise: crate console +7
decision: tool results are coloured from a fresh parse at every break in the shown text (the first row, a line-number gap, an elision row, a clipped row, a parse error) instead of left plain when the parse cannot be sure | most reads start mid-file and every search hit is lifted out of its file, so the plain rule left nearly every result uncoloured | reversible by restoring the line-1 check in the tool card body
ledger+: TUI | every tool result is coloured: a read or edit by its file's language from any line it starts on, search hits and their context rows by each file's language under the file's path, a streaming write by its path and a streaming cell as Python, other output by what it holds (JSON, a diff, a shebang), the colour codes a program printed as colour, and file paths, `path:line` included, in the status-path colour; the console and solo notebooks paint cell output the same way
---
Tool results are coloured wherever they appear. A read that starts in the middle of a file, the
tail after a long result's `… N more lines` row, and every search hit are coloured by the file's
language. A search groups its hits and context rows under the file's path and drops the anchor
tag. Shell and notebook output shows the colours the program printed instead of `␛[31m`, and any
other escape sequence is dropped. JSON, diffs and scripts with a shebang are typed by their
content, and file paths, `path:line:col` included, take the theme's status-path colour.

The parse restarts from the grammar's first state at every break in the shown text, so a row
that opens inside a string or a comment colours wrong at most until the next break. Painted card
bodies are kept per thread, keyed on the card's text and the theme, since a 19-row Rust read
costs about 2.3 ms of syntect and the console repaints every visible card; a repaint now costs
about 40 µs.
