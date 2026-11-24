---
version: 0.1084.0
decisions: D611
issue: Closes #2304
decision: in Yi Desktop a pane shows a thing of its session and exists only while there is one, a layout sets places and not panes, and what opens by itself arrives as a quiet tab that never takes focus, under one show action shared by the person, a rule table and one model tool that says what to show and not where | the owner found that "too many blank panels are being opened with low user value" and asked that "panels should be session linked", then chose each of these in two question rounds on 2026-11-21 | remove `Pane.seated`, `Hole.resting`, `GridMsg::Have` and `yd_panes::has`, and a layout seats every pane it names again
---
In Yi Desktop a saved layout, and a layout a person kept, now seats only the panes that have something to show. Picking Everything in a session with nothing in it draws the chat alone; Changes takes its own place in the layout when the agent first edits a file, the terminal when the agent first runs a command, the web pane when a page opens. A pane that is waiting keeps its slot and the grid closes up round it, so there is no hole and no half-pane split when it arrives. A pane the person opens by hand is theirs and stays whatever it holds. `desktop/docs/layout.md` holds the whole design, of which this is the first of six stages (#2305 to #2309 are the rest).
