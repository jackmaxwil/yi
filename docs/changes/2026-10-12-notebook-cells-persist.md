---
growth: +244 the per-session cell store in `ConsoleState::notebooks` with `apply_notebook`'s caps (stdout 8 KiB, result 2 KiB, at most 4 images of at most 64 KiB each) and the one `notebook_cells` lookup shared by render, the idle close, the interrupt arm and the image follow; weighed dropping a session's cells when its pane closes (the incident is that they must survive) and truncating a base64 image in place (a cut payload is a corrupt PNG)
raise: crate console +244
---
Notebook cells outlive the pane. The cells lived inside the notebook pane, so closing it (by hand or by the idle auto-close) and reopening showed "no kernel cells yet" while the session's kernel still held every variable, and a cell that ran while the pane was closed was never recorded. They now sit in `ConsoleState::notebooks` per session beside the review diffs; the pane renders from that store, and `apply_notebook` records every kernel tool call whether or not a pane is open.
