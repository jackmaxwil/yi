---
issue: Closes #1739
raise: crate console +132, crate tui +150, tests +153, comments +4
growth: +248 one traceback classifier shared by the notebook and the chat's ipython card, with the three tests that fail without it; weighed folding the pane's own cap copy, the `chip` export and the bash-cell test, and kept them because a copy is what this removes and the tests are the red proof
---
The console notebook pane drew a traceback as one flat wall: every row the same error colour
with the `✕` marker welded to its front, so the frame that raised and the line it ran read as
nothing, and a `%%bash` cell's source was lexed by the Python grammar the pane hardcoded, which
painted `ls` and `head` as plain words and `-5` as a Python number.

`yi_tui::pycell::traceback_spans` classifies a traceback's rows once, for both the console
notebook and the chat's ipython card: a `File …, in f` or `Cell In[n], line N` header and the
dashes above it are scaffolding and dim, the `----> n` row and the numbered rows around it are
the code that frame ran and go through the one shared highlighter (`yi_tui::highlight::spans`),
and the exception row is the one row wearing the `✕` and the bold error colour, so a tail-cut
block still says on screen what failed. The kernel's numbering and arrow stay part of the dim
gutter rather than being lexed as Python operators.

The pane keeps its caps: the traceback block still cuts to its last 12 rows behind the same
`[…] traceback last 12 of N rows · notebook cap 12 · v shows every row` notice, now emitted by
one `cap_rows` that both the plain streams and the traceback share. `yi_tui::pycell::chip` is
public so the cell's source and its traceback pick the same grammar, bash for a `%%bash` cell.

The rows are built once per width behind the pane's cache, so highlighting a long cell costs the
frame that changes the width, not every frame.
