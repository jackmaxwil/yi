---
issue: Refs #2255
shares: #2250 crates/tui/src/port.rs and crates/tui/src/render.rs each take one addition here: a reply arm for the rewrite answer, and the composer's footer draw call
shares: #2239 crates/console/src/app/chat.rs takes one match arm here, so the exhaustive command match still compiles
shares: #2237 crates/runtime/src/lib.rs takes one `pub mod rewrite;` line here
shares: #2205 crates/types/src/config.rs takes the `rewriter` role and the `rewrite` config types here
growth: +889 the rewriter is 355 runtime lines (the rules text the trial ran, the excerpt with its named cut, one deadline-bound call, 85 lines of tests) and 485 TUI lines (the state table and marker with 115 lines of tests, and the Enter, Esc and key-lock wiring); the keys are 25 lines in types and 19 in the CLI, the console's loud refusal 5. Weighed for deletion and cut in design: a preview pane, a hidden session entry for the typed words, a bundled rewriter skill and a changed-file list; 29 doc comments that restated a signature were removed, and the composer's frame setter kept its signature so no existing test changed
raise: crate cli +19, crate console +5, crate runtime +355, crate tui +485, crate types +25, tests +353, comments +16
decision: A model role `models.rewriter` restates the composer's draft before it is sent, behind a control on the composer's bottom border with the positions off, short, medium and long; the rewrite is shown in the box before anything is sent, and the host appends the typed words under it as `My words, verbatim:` | the owner asked for "auto prompt improvement" with "visible chrome and button on/near the TUI composer. when button on, rewriter on" and "a slider or other picker so i can pick more concise or longer", chose "Auto, show me first" and "Quoted at the end, model sees it"; a ten-prompt trial showed a rewriter that reads the agent's answers turns them into the person's orders, so it reads the person's own last three messages only | unset `models.rewriter`
ledger+: TUI | with `models.rewriter` named, Enter restates the draft at the composer's `improve:` length, shows it over the typed words, and sends on a second Enter; Esc restores the typed words
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

The console draws no control yet: its sessions are reached over ACP, which has no rewrite call.
