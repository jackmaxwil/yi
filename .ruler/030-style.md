# Style: HAR compliance

The har* skills bundled with this repo are normative for every Rust line: har, har-api,
har-supply, har-verify always; the rest by their stated triggers. Enforced highlights:

- #![forbid(unsafe_code)] in every crate; panic budget zero from day one (clippy denies
  unwrap/expect/panic/todo/unimplemented; check_panic.py backs it).
- Newtypes for every id and unit (EntryId, SessionId, Tokens(u64), Bytes(usize)); no bare
  String/u64 across a crate boundary. Checked or saturating arithmetic on all budget, token,
  and offset math.
- A byte index is not a character index. A crate root carries `#![deny(clippy::string_slice)]`
  unless it is named in baselines/string_slice_pending.json, a list that only shrinks; there is
  no allow and no expect, a crate is clean or it is pending with an issue. `String::truncate` is
  a disallowed method: take `chars().take(n)`, `strip_suffix`, or walk back on `is_char_boundary`
  and say why. `error.truncate(80)` on a non-ASCII child error was a panic the zero-panic gate
  could not see: check_panic.py matches spellings, clippy matches behaviour.
- A field is written to be read. check_orphans.py fails a `pub` field outside yi-types that no
  line in the workspace reads (a serde-derived struct is read by the wire), and a baseline no
  script opens. Two context budgets, a status threshold and a 0-byte baseline were carried by
  nothing. A field only a test reads is kept with a reason or deleted.
- Typed errors (thiserror) at crate boundaries; wire-facing enums carry Other(String).
- A comment earns its line only by naming what the code cannot: the incident that created a
  constant or guard, an invariant the type system cannot express, or a schema fact on a
  yi-types public item. Restating a signature, a name, or the next three statements is none of
  these. The test is content, not sigil: /// is allowed wherever a comment is earned and banned
  where it is not. Two lines per comment, hard — only a license/attribution header may exceed
  it. If the fact needs three lines, it is two facts or it is narration. No narrative comments, no section banners, no commented-out code. check_comments.py backs
  it: the length cap outright, comment volume outside yi-types as a shrink-only ratchet.
- Comment referents are typed (D55). A Rust item named in a doc comment is an intra-doc link —
  [`AgentSession::attach_store`], [`crate::tools::ToolAdapter`] — never bare backticks;
  rustdoc::broken_intra_doc_links is denied and `cargo doc --document-private-items` runs in
  check_guardrails.sh, so a rename that misses the comment fails the build. Qualify the path:
  rustdoc resolves relative to the documented item's own module, so a bare method name usually
  will not resolve. Bare backticks then mean not-an-item, which is the common case and stays
  bare — a parameter (keep_recent), a wire name (display_data, sessionUpdate), a symbol in a
  reference codebase (convertToLlm). rustdoc resolves links only in /// and //!, so a comment
  inside a function body leaves the item bare; that is the ceiling, not an exemption.
- A comment claiming the incident or invariant grant says which: the first line opens
  `Incident:` or `Invariant:`. Closed vocabulary — check_comments.py rejects any other
  capitalized `Word:` prefix, which is what stopped Precedence:/Draining:/Detached: becoming a
  private dialect. Schema facts need no tag; crates/types/ is the tag. The prefix rides an
  existing line, so it costs no comment volume.
- A comment survives with no foreknowledge: strip every row and decision id from it and what
  remains must still fully carry the fact. Ids are optional trailing pointers, never the
  payload — "V12: standing constraints, append-only" tells a reader without the design doc open
  nothing at all, and it rots the day the row is renumbered. check_comments.py rejects a comment
  left with too little prose once its ids and pointer words are stripped.
- Fight for every line: the size ratchet is a ceiling, not a target.
- Multi-axis flows are state-space-as-data: a table the runtime reads, closed vocabulary,
  invariant checks — never a shadow model maintained beside a hand-rolled flow.

Wire-type drift corrections (these override instinct):

- Field declaration order IS the byte order of Pi-compatible output. Never
  reorder fields in a serialized struct; golden fixtures assert exact bytes.
- Numbers that may be integer-or-float on the JS side are `serde_json::Number`,
  never f64 — a stored `0` must not re-serialize as `0.0`.
- serde_json's `preserve_order` feature is load-bearing: arbitrary JSON objects
  (tool arguments, custom data) must round-trip with key order intact.
- Wire names are camelCase via rename_all plus exact tag strings; check the
  fixture before trusting a guess.
