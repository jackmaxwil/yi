---
issue: Closes #2305
---
In Yi Desktop a pane now belongs to the session it was opened in. Turning the window to another session rests that session's panes and shows the other's, with every split line where it was, because the window keeps one layout and each pane remembers the sessions it is open in. A pane about this Mac (Containers, Daemons, VMs, Plugins, Language servers, Assets) shows in every session, and "Keep Pane Across Sessions" in the Window menu does the same for any pane. Closing a pane closes it in this session and leaves it in the others. A new chat no longer clears the window's layout. A window file from before this opens with every pane kept across sessions, as it was drawn. This is stage 2 of `desktop/docs/layout.md`; the decision row is the one #2304 added.
