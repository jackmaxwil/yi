---
issue: Refs #2309
growth: +929 the tool and what it tells the model (runtime 241), the wait for a client's answer and the daemon's relay (acp 249), the console's answer with its address reader and its idle rule (console 332), the wire shapes (types 48) and the session builder's extra argument (cli 14), about four lines in ten being the unit tests beside them; weighed for deletion: the permission bridge's pending map and line sink carry the request instead of a second channel, the daemon's parking is skipped rather than extended, the console reuses its editor panel and its bottom-row notice instead of a new surface, the session keeps one slot for what a front end hands it where it had one for the rewriter, and an address reader shared with Yi Desktop through yi-types was rejected because yi-types holds shapes only
cut: none crates/acp/src/daemon.rs
decision: the model gets one tool, `show`: a file, optionally at a line, and a strength (`peek`, `tab`, `front`), and nothing else, so it cannot read the layout, close, move or resize. It is a request the client answers (`_yi/show`), so the model reads what happened, and one left unanswered for five seconds says so and the turn goes on. It is offered only in a session whose client said it can show things when it opened it (`_meta.yi.shows`), and both of Yi's clients answer it, the console and Yi Desktop | the owner chose "Says what, not where" on 2026-11-21 and approved this row on 2026-11-23 with one change: "this should be supported in both the desktop app and TUI. this is a durable principle. we must have parity between desktop app and tui"; asked whether one-in-one-out applies to a tool a terminal session never carries, the owner chose "No, nothing leaves"; Zed's follow-the-agent guesses from what a tool touched, where this is said on purpose and answered | remove the tool, `_yi/show` and `_meta.yi.shows`; each client keeps opening panes by its own rules
ledger=: model show tool | 7.1, 17.2, 17.4 | core | M | S | a session whose client set `_meta.yi.shows` on `session/new` or `session/resume` carries `show(address, strength)`, which sends that client `_yi/show` and tells the model the answer (`peek`, `tab`, `front`, `refused`, with the client's note) or that no display answered within five seconds; no other session carries its text, and its text is locked as `tool:show` outside the request budget; the console opens the file at its line in its editor panel, in front with the keys only for `front` after ten seconds with no key or mouse, else as a tab that waits with "the agent added <file> as a tab" on the bottom row, refuses an address that is no file and a session not on screen, and answers a request it does not know with -32601 where it used to answer nothing; the daemon never parks a show, tells the worker at once when no client is attached, and drops a client's error so another client's answer can win
raise: crate acp +350, crate cli +9, crate console +312, crate runtime +241, crate types +48, tests +725, comments +41
cut: none crates/acp/src/permission.rs
cut: none crates/console/src/app/editor.rs
cut: none desktop/crates/core/src/command/by_kind.rs
---

The model can now point at a file. In a session opened by the console, `show` with a path
(`src/main.rs`, `src/main.rs:40`, or `yi://file/src/main.rs#L40`) opens that file at that line
beside the chat. With strength `front` it comes forward with the keys only when no key or
mouse has been used for ten seconds; otherwise, and for `tab` and `peek`, it waits as a tab,
the bottom row says "the agent added main.rs as a tab", and the keys stay where they were. The
model reads which happened, or why not: the file is missing, the address is not a file, or
that session is not on screen.

`yi ask`, `yi rpc`, the solo `yi tui` and an editor over `yi acp` never see the tool: it joins
a session's tools only when the client that opened it set `_meta.yi.shows`, and it adds
nothing to their requests. Yi Desktop answers the same request in the change that follows.

A session already loaded in a worker keeps the tool set it was built with, so one built for a
client that cannot show does not gain the tool when a console attaches later.
