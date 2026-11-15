---
version: 0.1062.0
issue: #2227
cut: none crates/console/src/sidebar.rs
cut: none crates/tui/src/agents.rs
raise: crate tui +8, tests +34, comments +1
---
A plan child is named `<plan>/<todo>`, so the roster's 20-cell name column, the subagent start and end rules, the task cell title and the console sidebar all showed the same shared plan prefix for every child of one plan. Each now shows the part after the last slash, so every child reads under its own name.
