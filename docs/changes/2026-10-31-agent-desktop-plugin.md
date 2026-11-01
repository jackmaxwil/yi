---
issue: Refs #1958
growth: +706 about 260 lines are desktop.rs's test module and 170 the verb, app and input tables; weighed deleting the find, wait and list-surfaces rows, the x86_64 asset (deleted, never run) and a generic gate across plugins (not built, one program), and kept the tables since a missing row only asks more often
raise: comments +34
raise: tests +139
raise: crate cli +3
raise: crate permission +633
raise: crate runtime +17
raise: crate tools +29
raise: crate types +24
decision: computer use and browser automation enter scope through plugins, paid for by deleting the Python client, and a plugin that drives this machine leaves the sandbox only through a table of its verbs | the owner, 2026-10-31: "Any app on my Mac", "remove the design doc ban"; a typed command in Terminal gets around the sandbox, so the gate is the feature | delete the row and the table, restore the §1.2 clause
---
`yi plugin add agent-desktop` lets the model read and operate Mac apps through their accessibility trees, by bash, with no new tool. The sandbox refuses the program, even its `version`, so only its plain name leaves it, through a table of verbs and flags in `yi_permission`. A yes covers one app for the session, whether the command names it with `--app` or holds a ref, whose app comes from the record the snapshot wrote. A terminal, an editor, System Settings or a password app, and any verb or flag the table does not name, asks every time and keeps nothing. A browser is refused in every mode and pointed at agent-browser. So is whatever takes the user's own mouse, keyboard or front app (`--headed`, `launch --activate`, `press`, `hover`, `drag`, `mouse-*`, `double-click`, `batch`): the owner, 2026-10-31, "needs to not interrupt my mouse it needs to be its own agent mouse". `session start --cursor` runs unasked and draws an agent cursor upstream keeps apart from the real one. What the program prints returns inside the `yi-external` fence, and a review model never answers for it. The row's new `host` shape keeps it out of `yi plugin add --all` and the eval containers, and Apple silicon Macs are the only target, since no other build was run. The Python client is deleted, which the owner approved on 2026-10-13: "remove python client". §1.2's clause on browser automation, computer use and non-coding capabilities goes with it.
