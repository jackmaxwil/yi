---
raise: crate console +62
---
Notebook cells outlive the pane. The cells lived inside the notebook pane, so closing it (by hand or by the idle auto-close) and reopening showed "no kernel cells yet" while the session's kernel still held every variable, and a cell that ran while the pane was closed was never recorded. They now sit in `ConsoleState::notebooks` per session beside the review diffs; the pane renders from that store, and `apply_notebook` records every kernel tool call whether or not a pane is open.
