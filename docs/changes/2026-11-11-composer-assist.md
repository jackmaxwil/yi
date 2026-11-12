---
issue: Closes #2158
decision: The chat composer keeps its draft and draws assistance over it: a curly underline under a misspelled prose word, a gray ghost suffix with the cursor at the line's end, and lists for `/` verbs, `$` skills, `@` files, fetch schemes and emoji above the box. A list opens only on a typed edit (a paste or a recalled history entry opens none), Tab takes what is drawn and otherwise completes the token at the cursor, Enter on a `/` row runs it, Esc closes the list and keeps the draft, and an `ask_user` card that arrives takes the list's place at once; a replacement (a row, an emoji, a correction) is one ctrl+z that puts back what was typed, and each list that opens walks the folder and the skills again; the `/` and `@` popups and #2149's command-list popup go, and skills list under `$` only, so no skill can shadow a verb | the bottom stack was a composer or a view that replaced it, so a list hid the draft and a paste landed in the hidden box; one list over the draft serves every token kind | restore `Bottom::Command` and `Bottom::File` in crates/tui/src/app.rs and their key handling in input.rs
decision: Spelling is Apple's NSSpellChecker, reached by a long-lived `/usr/bin/osascript -l JavaScript` peer that yi-tools spawns through its `command` constructor (design §9) and speaks one ASCII-escaped JSON object a line with a request id; the composer only asks and polls a non-blocking cache, a peer that misses a 3 s deadline, desyncs or cannot start leaves spelling absent, and nothing is downloaded, compiled or shipped beside `yi`. Off macOS there is no checker and completion has the user's own prompt words only, in solo and the console alike; a headless drive installs neither, so its frames never depend on the machine. A late or dead peer turns spelling off for the session and says so in one notice row, and the curl is written only where the terminal advertises true color, a plain underline elsewhere | owner 2026-11-11: "Can we use NSSpellChecker … so we dont need to download model?"; Rust cannot call AppKit without unsafe FFI, and a helper compiled in build.rs would need release packaging, a target-arch build and a path found from `current_exe`, where osascript ships on every Mac; measured on this Mac, 61 ms to the first answer and 1.8 ms check, 2.5 ms complete, 3.7 ms guesses at the median | delete crates/tools/src/spell.rs and `Assist`'s engine; typos and Apple completion then stay off
decision: Esc and Ctrl-C close a completion list and keep the draft, superseding #2149's "Esc and Ctrl-C close the list and empty the box, as before" | with the draft always in the box, keeping it is the one rule a typed list and a Tab-opened list share, where #2149 emptied the one and kept the other; coordinator's call for this PR, 2026-11-12 | make `Composer::dismiss_assist` clear the text when a typed list closes
ledger+: TUI | the composer stays on screen under typo curls (macOS spelling), a ghost suffix and lists for `/`, `$`, `@`, schemes and emoji; a list opens only on typing
growth: +1645 the speller is 282 lines in yi-tools and the composer's assist 1257 in yi-tui, round 2's one-step undo, dead-checker notice and true-color gate among them; weighed for deletion and cut: the `tui.assistRows` knob, three of four token walkers, the per-process file and skill cache, the small-model download and `yi predict` (-55 cli, -120 engine), the compiled `spell.m` helper and its build step (-157), the question card's own assist (-113), the usage-ranked slash ledger, the test-only engine switches (`fake_engine`, `observe`, four thread-locals) and the dead file-walk `searching` path; kept: emoji and scheme rows (the owner's list) and the session word list, the only completion off macOS
raise: crate cli +12, crate console +56, crate runtime +25, crate tools +282, crate tui +1257, crate types +14, comments +57, tests +695
cut: none crates/console/src/app/chat.rs
cut: none crates/console/src/lib.rs
cut: none crates/tui/src/app.rs
cut: none crates/tui/src/colors.rs
cut: none crates/tui/src/commands.rs
cut: none crates/tui/src/highlight.rs
cut: none crates/tui/src/input.rs
cut: none crates/tui/src/markdown.rs
cut: none crates/tui/src/render.rs
cut: none crates/tui/src/status.rs
shares: #2150 the terminal-tab lane touches the same console chat and TUI render files; neither depends on the other
shares: #2154 custom providers add config keys and settings rows beside this branch's `tui` keys
shares: #2155 a separate YI_DESIGN section
shares: #2153 a separate YI_DESIGN section
shares: #2149 the queue branch, merged here; its command-list popup is replaced by the composer's list
shares: #2138 the theme branch this one stacks on
---

The chat composer keeps its draft on screen and draws assistance over it: a curly underline in the error color under a misspelled prose word, a gray suffix after the cursor that Tab accepts, and lists above the box for slash commands, `$` skills, `@` files, the fetch schemes Yi already has, and emoji shortcodes. A list opens only on a typed edit, so a pasted draft or a recalled history entry opens none and the arrows keep walking history. Tab takes what is drawn; with nothing drawn it completes the token at the cursor, in place for one match and as a list for several. Enter on a `/` row runs that command. Esc closes a list and keeps the draft. A `:name:` typed closed in prose becomes its emoji; code, paths and `std::x::y` keep their colons. `ctrl+.` lists spelling guesses where the terminal can send it.

On a Mac the spelling is Apple's own checker, which ships with the system, reached through `osascript` in the background so typing never waits on it; nothing is downloaded. Elsewhere there are no typo marks, and completion uses the words of the user's recent prompts, in solo and the console alike; a headless drive has neither. If the checker fails or answers late, spelling is off for the session and one notice row says so. Autocorrect defaults off, and one ctrl+z undoes any replacement whole.

Based on Max Ludwick's GitHub jackmaxwil/yi#9 (forge draft #2139).
