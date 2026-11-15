---
version: 0.1060.0
issue: #2229
ledger+: console editor and viewers | a split or a new tab opens a chat in the project of the pane it came from (`app::menu::tests::a_new_pane_or_tab_opens_chat`, T0)
shares: #2240 changes three action arms in crates/console/src/app.rs, away from the MCP pane arms
shares: #2239 changes three action arms in crates/console/src/app.rs, away from the ACP type paths
shares: #2167 changes three action arms in crates/console/src/app.rs, away from its lines
raise: crate console +55, tests +40, comments +1
---
A split right, a split down and a new tab each open a chat in the new pane, in the project of the pane that had focus, instead of an empty pane the person had to start a session in by hand.
