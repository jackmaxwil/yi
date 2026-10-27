---
issue: Closes #1502, Closes #136, Refs #804, Refs #807, Refs #1442
growth: +137 typed errors, block/read validation, anchor and I/O safety after splitting read arguments out of tool.rs and deleting string-slice and prefix-classification code
raise: tests +226, crate tools +123, crate runtime +13
---
This change combines HB-utf8-tools, HA-EDIT-typed, HA-EDIT-blocks, HA-EDIT-readargs, HA-EDIT-anchor, and HA-EDIT-mvread.

Hashline parsing denies byte-indexed string slices in yi-tools and uses character-boundary-safe access throughout its tokenizer, input recovery, parser, metadata handling, and multiline grep numbering. Patch failures retain their existing display text while gaining typed variants for callers. Reclaim recognizes only bracketed path headers that carry a snapshot tag.

Block resolution covers multiline Python signatures and Rust lifetimes without changing one-line definitions or character literals. Read handles existing bracketed paths before glob routing, validates numeric string arguments, and refuses a zero limit or multiline find needle with its rule.

Long displayed rows can anchor adjacent inserts while replacement still requires full source text. Addressable line counts exclude the trailing split artifact, moves refuse to overwrite an existing destination, and unreadable files report their actual read failure instead of inviting an overwrite.
