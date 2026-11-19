---
issue: Refs #2255
shares: #2250 crates/tui/src/port.rs and crates/tui/src/render.rs each take one addition here: a reply arm for the rewrite answer, and the composer's footer draw call
shares: #2239 crates/console/src/app/chat.rs takes one match arm here, so the exhaustive command match still compiles
shares: #2237 crates/runtime/src/lib.rs takes one `pub mod rewrite;` line here
shares: #2205 crates/types/src/config.rs takes the `rewriter` role and the `rewrite` config types here
cut: none crates/console/src/app.rs
growth: +1279 the rewriter is 355 runtime lines (the rules text the trial ran, the excerpt with its named cut, one deadline-bound call, 85 lines of tests); the TUI is 619 lines (the state table and marker, the pickable positions and geometry test, the Enter, Esc and key-lock wiring); the console and ACP carry the same control to a daemon-held session: 61 acp lines for `_yi/rewrite` and the rewriter in the config update, 198 console lines for sending it, answering it, learning the rewriter, and a click target per position; keys are 28 types lines and 19 CLI lines. Weighed for deletion and cut in design: a preview pane, a hidden session entry for the typed words, a bundled rewriter skill and a changed-file list; 29 doc comments that restated a signature were removed, the deadline helper moved out of the capped console file, and the composer's frame setter kept its signature so no existing test changed
decision: A model role `models.rewriter` restates the composer's draft before it is sent, behind a control on the composer's bottom border with the positions off, short, medium and long; the rewrite is shown in the box before anything is sent, and the host appends the typed words under it as `My words, verbatim:` | the owner asked for "auto prompt improvement" with "visible chrome and button on/near the TUI composer. when button on, rewriter on" and "a slider or other picker so i can pick more concise or longer", chose "Auto, show me first" and "Quoted at the end, model sees it"; a ten-prompt trial showed a rewriter that reads the agent's answers turns them into the person's orders, so it reads the person's own last three messages only | unset `models.rewriter`
ledger+: TUI | with `models.rewriter` named, Enter restates the draft at the composer's `improve:` length, shows it over the typed words, and sends on a second Enter; Esc restores the typed words
raise: crate acp +61, crate cli +19, crate console +198, crate runtime +353, crate tui +620, crate types +28, tests +617, comments +31
---

Naming `models.rewriter` puts a control on the composer's bottom border:
`improve: off · short · [medium] · long`, stepped with `alt-i`. At any position but off, Enter
sends the draft to that model, and the box then holds the rewrite with the typed words quoted
under it. A second Enter sends it and Esc puts the typed words back. A rewrite that fails or
runs past 20 seconds leaves the draft to go as typed, with the reason on a notice row.

`rewrite.skill` names a skill whose body guides the rewriter after its own rules,
`rewrite.length` sets where a session starts, and `rewrite.thinking` sets its thinking level
(low when unset). The rewriter reads the draft, the person's last three messages and the
folder's git line, and never the agent's answers.

The console draws the same control on each chat pane and each position is a click target. A daemon-held session reaches the rewriter over a new ACP method, `_yi/rewrite` (`sessionId`, `length`, `draft`), answered with `{"text"}` or `{"reason"}`, and announces its start length in the `_yi/config` update and in `_meta.yi.rewriter` of a session result.
