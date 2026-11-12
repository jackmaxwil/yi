---
decision: The chat composer keeps its draft and draws assistance over it, with typo underlines, a ghost suffix, and completion lists for slash commands, files, skills, fetch schemes and emoji, while the word engines stay outside the yi link | the bottom stack was a composer or a view that replaced it, so a list hid the draft and a paste could land in the hidden box | restore the takeover popups in crates/tui/src/input.rs
ledger+: TUI | the composer stays on screen under typo underlines, a ghost suffix and completion lists
growth: +2053 weighed linking the spelling helper and the small model into yi; both stay a separate program
raise: crate cli +64, crate tui +1973, crate types +52, crate console +38, tests +281, comments +3
---

The chat composer keeps its draft on screen and draws assistance over it: a curly underline in the error color under a misspelled prose word, a ghost suffix after the cursor, and a completion list for slash commands and skills, files, the fetch schemes Yi already has, and emoji shortcodes. Esc closes the list and leaves the draft. Tab accepts a visible suggestion. Word completion defaults to auto, which blends an n-gram of this machine's prompts with Apple spelling on macOS and a small downloaded model when one is present. Autocorrect defaults off. The Apple checker and the small model run in a helper outside the yi link, so `yi --version` does not start them.
