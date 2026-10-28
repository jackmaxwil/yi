---
issue: Closes #1502, Closes #136, Refs #804, Refs #807, Refs #1442
growth: +182 typed errors, block/read validation, displayed-clipped anchor metadata and I/O safety after splitting read arguments out of tool.rs and deleting string-slice and prefix-classification code
raise: tests +342, crate tools +156, crate runtime +26
---
This change combines HB-utf8-tools, HA-EDIT-typed, HA-EDIT-blocks, HA-EDIT-readargs, HA-EDIT-anchor, and HA-EDIT-mvread.

Hashline parsing denies byte-indexed string slices in yi-tools and uses character-boundary-safe access throughout its tokenizer, input recovery, parser, metadata handling, and multiline grep numbering. Patch failures retain their existing display text while gaining typed variants for callers. Reclaim recognizes only bracketed path headers that carry a snapshot tag.

Block resolution covers multiline Python signatures, including single-quoted defaults, and Rust lifetimes without changing one-line definitions or character literals. Read handles existing and tagged bracketed paths before glob routing, validates numeric string arguments, and refuses a zero limit or multiline find needle with its rule.

Ordinary unseen gap anchors remain refused. Only displayed clipped rows can anchor adjacent inserts and register pastes without their full text; every replacement still requires shown lines. Addressable line counts exclude the trailing split artifact, moves refuse to overwrite an existing destination, and unreadable files report their actual read failure instead of inviting an overwrite.
