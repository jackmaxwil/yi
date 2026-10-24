---
issue: Closes #1469, Closes #138, Closes #139, Closes #140, Closes #141, Refs #1442
raise: crate ai +2, crate kernel +1, tests +50
---
This change bundles three units: HB-utf8-rest, HA-INT-abort and HA-PLAN-leak.

HB-utf8-rest: yi-ai, yi-context, yi-kernel and yi-mcp-cli now deny `clippy::string_slice` at their crate roots, so no byte-index slice of a `str` compiles under clippy in those crates. The fifteen slices they held were boundary-safe, each behind a `find` of an ASCII byte or a hand-written walk back to a char boundary; they are now `split_once`, `str::get`, or `floor_char_boundary` and `ceil_char_boundary`, which deletes the five hand-written walks. The pending list keeps only runtime and tools, which other changes clear.
