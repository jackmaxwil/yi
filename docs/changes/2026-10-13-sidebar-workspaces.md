---
issue: Closes #1376
growth: +315 measured from the fork, the stacked #1371 chrome change included; this change's share is the daemon's `_yi/pin` (+27), the ledger field (+4) and the console's groups, pins and header controls; deleted for it the workspaces block, the root filter and its ←/→ walk, the inbox section headers and the empty-sidebar fallback; weighed and kept the folded header's needs-you mark (a fold must not hide a session waiting on the user) and the `cwd` that lets a pin create a ledger row (after a restart most listed sessions are the worker's alone)
raise: tests +265, comments +4, crate console +137, crate acp +27, crate types +4
decision: the console's full sidebar groups sessions by workspace under a PINNED section, in place of D276's needs you / working / idle sections and the separate workspaces block (amends D276) | the owner asked to "organize by workspace + pinned. Copy the claude code sidebar organization and layout" and chose "Needs-you first, then recent (Recommended)" inside a group (#1376); the inbox mixed every root's sessions into one need-ranked list, and the workspaces block was a filter beside it rather than the list's own structure | restore the section headers in `sidebar_lines`, the need-then-recency sort across roots in `visible_rows`, `render_roots` with its `root_filter`, and drop `groups`, `folded` and `_yi/pin`; a ledger written meanwhile keeps `pinnedMs`, which an older daemon reads as an unknown field
---
The console's full sidebar is laid out like Claude Code's (Closes #1376). It opens with
`+ New session` and `⌕ Search`, each naming its key; a click on either starts a session or
opens the palette. PINNED follows with the pinned sessions in the order they were pinned, then
one group per workspace, the console's own root first. A workspace's header carries its name, a
`▾`/`▸` fold mark and a `+` at the row's end that starts a session in that root; a click on the
rest of the header folds the group for this console only, and a folded header still shows `?`
or `●` when a session in it needs you. Inside a group, sessions rank by need, then recency. A
session is pinned or unpinned from its right-click menu, with ⌥p, or with ⌘⇧K (⌘⇧P is
Ghostty's and kitty's); the daemon keeps the pin in its ledger (`pinnedMs`, the pin's time), so
it survives a restart, and answers it to every console in `session/list`. The rail keeps the
same order, pinned first, with a blank row between workspaces. The separate workspaces block
and the ←/→ workspace filter are gone: the groups are the workspaces. Every new row lifts under
the pointer.
