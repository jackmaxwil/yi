---
issue: #2229
raise: crate console +46, tests +30, comments +1
ledger+: console editor and viewers | a split or a new tab opens a chat in the project of the pane it came from (`app::menu::tests::a_new_pane_or_tab_opens_chat`, T0)
---
A split right, a split down and a new tab each open a chat in the new pane, in the project of the pane that had focus, instead of an empty pane the person had to start a session in by hand.
