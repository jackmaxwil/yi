---
issue: Closes #2145
growth: +190 the store scan with its loud cap, the gate's descriptor check and the belt's alias check, split into catastrophic/alias.rs since catastrophic.rs may not grow past 1,000 lines; weighed and deleted: the import's three tree walks, its rg flag table and duplicate hard-link predicate (the searched tree is never walked now), and the tools crate's copy of `links`, which now comes from yi-permission (-10)
raise: crate permission +200, tests +174, comments +13
cut: none crates/permission/src/catastrophic.rs
shares: #2136 forge draft #2136 is Max's whole mirror branch, which carries the first version of this gate; this PR replaces that part of it, and #2136 is being split
shares: #2137 stacked on #2136, so it carries the same first version of this gate in the permission files
shares: #2150 YI_DESIGN.md only: this branch edits the sandbox section's read-belt sentence, it edits the console and terminal-tab sections
shares: #2139 YI_DESIGN.md only: this branch edits the sandbox section's read-belt sentence, it edits the composer and dist-profile sections
shares: #2135 YI_DESIGN.md only: this branch edits the sandbox section's read-belt sentence, it edits the terminal-tab and feature sections
shares: #2134 YI_DESIGN.md only: this branch edits the sandbox section's read-belt sentence, it edits the console section
decision: A hard link to a file inside a credential store is refused by identity: the read gate checks the file it opened, and the bash read belt asks for a path argument that is one, or for a glob or a recursive search (rg, ag, ack, a recursive grep) while any store file has a name outside the stores; the stores are read whole up to `STORE_SCAN_CAP` entries, past which a hard-linked read asks and names the cap, and the searched tree is never walked (amends D323 and D339, which left the hard link open) | a hard link outside a store reads a key past the gate's spellings and past Seatbelt, which matches the name opened; review of #2136 found a belt that walked the searched tree refusing `rg foo` after 12-14 s in a large repo, and failing closed on any unreadable directory | drop `Aliases` from `ReadGate` and `reads_alias` from `command_reads_credentials`
ledger+: permission modes/rules/holds, deterministic auto | a hard link to a file inside a credential store is refused by identity at the read gate's open, and the bash read belt asks for one named, or for a glob or recursive search while a store file has a name outside the stores, never walking the searched tree; past 10,000 store entries (`STORE_SCAN_CAP`) a hard-linked read asks naming the cap (`open::a_hard_link_to_a_key_file_is_refused`, `decide::recursive_search_cannot_walk_to_a_hard_linked_credential`, `open::a_store_scan_reads_to_its_cap_and_names_it_past_that`)
---
A hard link outside a credential store no longer reads a key. `read`, `grep`, `edit` and every
other tool that opens through the read gate refuse a file whose identity is a store file, judged
on the descriptor they opened, so a link swapped in after the check is refused too. In bash, a
path argument that is such a link asks, and so does a glob or a recursive search (`rg`, `ag`,
`ack`, `grep -r`) while any store file has a name outside the stores, since the sandbox matches
the name opened and cannot see the link. The searched tree is never walked: the stores are, once
per check and only when a file with two names shows up, so `rg foo` in a large repo runs as
before. A store too big to read whole (over 10,000 entries) makes every hard-linked read ask and
says so. Contributed by Max Ludwick (GitHub #6, forge draft #2136).
