---
issue: Closes #2145, Refs #2156
growth: +186 the store scan with its loud cap, the gate's descriptor check and the belt's every-command ask while a key has an outside name, split into catastrophic/alias.rs since catastrophic.rs may not grow past 1,000 lines; weighed and deleted: the import's three tree walks, its rg flag table, its duplicate hard-link predicate, round one's lexical searcher and glob lists (the sandbox cannot see a link, so words cannot judge it), and the tools crate's copy of `links` (-10); grep's refusal row names a cut in net-zero lines, since grep.rs is past 1,000
raise: crate permission +196, tests +303, comments +16
cut: none crates/permission/src/catastrophic.rs
shares: #2136 forge draft #2136 is Max's whole mirror branch, which carries the first version of this gate; this PR replaces that part of it, and #2136 is being split
shares: #2137 stacked on #2136, so it carries the same first version of this gate in the permission files
shares: #2150 YI_DESIGN.md only: this branch edits the sandbox section's read-belt sentence, it edits the console and terminal-tab sections
shares: #2139 YI_DESIGN.md only: this branch edits the sandbox section's read-belt sentence, it edits the composer and dist-profile sections
shares: #2135 YI_DESIGN.md only: this branch edits the sandbox section's read-belt sentence, it edits the terminal-tab and feature sections
shares: #2134 YI_DESIGN.md only: this branch edits the sandbox section's read-belt sentence, it edits the console section
shares: #2152 YI_DESIGN.md only: this branch edits the sandbox section's read-belt sentence, it edits the console section
decision: A hard link to a file inside a credential store is refused by identity. The read gate checks the descriptor it opened. While a complete scan of the stores finds a store file with a name outside them, every bash command asks, since the sandbox matches the name opened; the ask is its own kind, so an approved or rule-allowed call stays contained. While the scan is cut (past `STORE_SCAN_CAP` entries, or a store that cannot be read), only a named path with more than one name asks, and the ask and the read gate name the cap or the path. The searched tree is never walked (amends D323 and D339, which left the hard link open). Left open: a single-file bind mount of a store file, which keeps one link; a store file created after a gate's scan, for the rest of that one call; an APFS clone, which is a new file; and the target of a symlink inside a store, readable by its own name (#2156) | a hard link outside a store read a key past the gate's spellings and past Seatbelt; review of #2136 found a belt that walked the searched tree refusing `rg foo` after 12-14 s, and review of #2153 found that an alias hit took the call out of the sandbox and that a lexical belt missed `cd`, `xargs` and `git grep` | drop `Aliases` from `ReadGate` and the `command_reads_alias` ask from `decide`
ledger+: permission modes/rules/holds, deterministic auto | a hard link to a file inside a credential store is refused by identity at the read gate's open; while a store file has a name outside the stores every bash command asks and stays contained once approved, and past 10,000 store entries (`STORE_SCAN_CAP`) or an unreadable store only a named two-name path asks, naming the cut, as grep's refusal row does (`open::a_hard_link_to_a_key_file_is_refused`, `decide::a_key_with_a_name_outside_the_stores_makes_every_command_ask`, `open::a_store_scan_reads_to_its_cap_and_names_it_past_that`, `permission_scope::a_hard_linked_key_keeps_an_allowed_search_contained`)
---
A hard link outside a credential store no longer reads a key through a tool, and no longer
reads one unasked through bash. `read`, `grep`, `edit` and every other tool that opens through the
read gate refuse a file whose identity is a store file, judged on the descriptor they opened, so a
link swapped in after the check is refused too; grep's refusal row counts those files apart and
names a cut scan when there is one. Bash cannot be judged by its words, since the sandbox matches
the name a process opens and `cd`, `xargs`, `git grep` or a script reach the link unnamed. So while
a store file has a name outside the stores, every bash command asks, naming the file, and an
approved call still runs inside the sandbox. Removing the outside name ends it. The searched tree is
never walked; only the stores are, once per check, so `rg foo` in a large repo runs as before. If
the stores hold more than 10,000 entries or one cannot be read, only a named path with two names
asks, and the ask says why. Kernel Python and an uncontained run are not judged by this. Contributed
by Max Ludwick (GitHub #6, forge draft #2136).
