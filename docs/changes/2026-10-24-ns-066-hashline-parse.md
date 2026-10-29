---
issue: Closes #1502, Closes #136, Refs #804, Refs #807, Refs #1442
growth: +332 typed errors, language-aware block/read validation, displayed-clipped anchor metadata and I/O safety after splitting read arguments out of tool.rs and deleting string-slice and prefix-classification code
raise: crate runtime +232, crate tools +306, tests +876, comments +13
---
This change combines HB-utf8-tools, HA-EDIT-typed, HA-EDIT-blocks, HA-EDIT-readargs, HA-EDIT-anchor, and HA-EDIT-mvread.

It also moves `YI_TRACE`'s production file sink out of the `yi` binary into `yi_runtime::trace_sink`, which is not hashline work: the journals change's plan-journal trace test runs inside yi-runtime, may not depend on yi-cli, and so carried a second copy of the sink in the test. The review round asked for the real sink to be shared rather than duplicated, and the journals change file carries the runtime lines that move adds (+166 to +206).

Hashline parsing denies byte-indexed string slices in yi-tools and uses character-boundary-safe access throughout its tokenizer, input recovery, parser, metadata handling, and multiline grep numbering. Patch failures retain their existing display text while gaining typed variants for callers. Reclaim recognizes only bracketed path headers that carry a snapshot tag.

Block resolution covers multiline Python signatures, including single-quoted defaults, and Rust lifetimes without changing one-line definitions or character literals. Read handles existing and tagged bracketed paths before glob routing, validates numeric string arguments, and refuses a zero limit or multiline find needle with its rule.

Non-Rust block comments keep their delimiters out of block depth across line boundaries, and comment markers inside regular-expression classes do not swallow the rest of a line.

Ordinary unseen gap anchors were already refused before this change: the parent collects a gap anchor and the unseen-line guard refuses it. The new behaviour is the clipped-row carve-out alone, and `hashline_patcher::a_clipped_row_anchors_an_adjacent_insert_but_not_its_replacement` is the test that proves it. `hashline_patcher::unseen_gap_anchors_are_refused_without_changing_the_file` guards only that the carve-out leaves unseen gap anchors refused; it passes on the parent too. Only displayed clipped rows can anchor adjacent inserts and register pastes without their full text; every replacement still requires shown lines. Addressable line counts exclude the trailing split artifact, moves refuse to overwrite an existing destination, and unreadable files report their actual read failure instead of inviting an overwrite.
