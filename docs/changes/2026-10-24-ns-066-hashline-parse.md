---
issue: Closes #1502, Closes #136, Refs #804, Refs #807, Refs #1442
growth: +345 typed errors, language-aware block/read validation, displayed-clipped anchor metadata and I/O safety after splitting read arguments out of tool.rs and deleting string-slice and prefix-classification code
raise: crate runtime +192, crate tools +319, tests +934, comments +13
---
This change combines HB-utf8-tools, HA-EDIT-typed, HA-EDIT-blocks, HA-EDIT-readargs, HA-EDIT-anchor, and HA-EDIT-mvread.

Hashline parsing denies byte-indexed string slices in yi-tools and uses character-boundary-safe access throughout its tokenizer, input recovery, parser, metadata handling, and multiline grep numbering. Patch failures retain their existing display text while gaining typed variants for callers. Reclaim recognizes only bracketed path headers that carry a snapshot tag.

Block resolution covers multiline Python signatures, including single-quoted defaults, and Rust lifetimes without changing one-line definitions or character literals. Read handles existing and tagged bracketed paths before glob routing, validates numeric string arguments, and refuses a zero limit or multiline find needle with its rule.

Non-Rust block comments keep their delimiters out of block depth across line boundaries, and comment markers inside regular-expression classes do not swallow the rest of a line. Shell scripts, makefiles and extensionless files such as `Makefile` or `bin/deploy` keep plain brace scanning, so a glob like `/var/backups/*` does not open a block comment.

Ordinary unseen gap anchors were already refused before this change: the parent collects a gap anchor and the unseen-line guard refuses it. The new behaviour is the clipped-row carve-out alone, and `hashline_patcher::a_clipped_row_anchors_an_adjacent_insert_but_not_its_replacement` is the test that proves it. `hashline_patcher::unseen_gap_anchors_are_refused_without_changing_the_file` guards only that the carve-out leaves ordinary unseen gap anchors refused; it passes on the parent too, so it is a regression guard and not a red proof. Only displayed clipped rows can anchor adjacent inserts and register pastes without their full text; every replacement still requires shown lines. Addressable line counts exclude the trailing split artifact, moves refuse to overwrite an existing destination, and unreadable files report their actual read failure instead of inviting an overwrite.
