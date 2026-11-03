---
version: 0.857.0
decisions: D524
issue: Refs #1966
decision: A feature-ledger edit is a line in the change file, applied by the recorder (extends D342) | 43 of 100 conflicted main merges touched a ledger row, a 2-4 KB line every feature appends to, and clause-per-line files clear only 17 of those 43 because most row edits replace text | Let branches edit rows again by dropping the `ledger-edit` check; the lines already recorded stay applied
---
A change file now carries `ledger+`, `ledger~`, `ledger=` and `ledger-` lines, which the recorder applies to the feature ledger after the merge, so two PRs editing one row no longer conflict. A branch that edits a row directly is refused and shown the lines to write, unless the edit was committed before this landed. A line that no longer applies is named on the recorder's output and its text kept on the row.
