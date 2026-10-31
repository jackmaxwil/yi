---
version: 0.825.0
issue: Closes #1739
raise: crate console +207, crate tui +161, tests +175, comments +9
growth: +368 one traceback classifier and the pane's cap helpers, with the three tests that fail without it; weighed folding the pane's own cap copy, the `chip` export and the bash-cell test, and kept them because a copy is what this removes and the tests are the red proof
---
The console notebook pane drew a traceback as one flat wall: every row the same error colour
with the `✕` marker welded to its front, so the frame that raised and the line it ran read as
nothing, and a `%%bash` cell's source was lexed by the Python grammar the pane hardcoded, which
painted `ls` and `head` as plain words and `-5` as a Python number.

`yi_tui::pycell::traceback_rows` classifies a traceback's rows once: a `File …, in f` or
`Cell In[n], line N` header and the dashes above it are scaffolding and dim, the `----> n` row and
the numbered rows around it are the code that frame ran and go through the one shared highlighter
(`yi_tui::highlight::spans`), and the exception row is the one row wearing the `✕` and the bold
error colour. A bare `AssertionError`, which a failed `assert` prints with no value after it, is
the block's last row and raises like any other exception, so the marker names what the reader came
to see even when the block is tail-capped. The kernel's numbering and arrow stay part of the dim
gutter rather than being lexed as Python operators.

The pane keeps its caps: the traceback block still cuts to its last 12 rows behind the same
`[…] traceback last 12 of N rows · notebook cap 12 · v shows every row` notice, placed by the one
`with_notice` the plain streams' cap calls too, so which end a notice sits on has one
implementation. Classification is the only pass over every row; the spans and the wrapping are
built for the 12 rows the cap keeps, so a long traceback costs what is on screen.
`yi_tui::pycell::chip` is public so the cell's source and its traceback pick the same grammar,
bash for a `%%bash` cell.
