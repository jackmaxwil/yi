# Style: HAR compliance

The har* skills bundled with this repo are normative for every Rust line: har, har-api,
har-supply, har-verify always; the rest by their stated triggers. Enforced highlights:

- #![forbid(unsafe_code)] in every crate; panic budget zero from day one (clippy denies
  unwrap/expect/panic/todo/unimplemented; check_panic.py backs it).
- Newtypes for every id and unit (EntryId, SessionId, Tokens(u64), Bytes(usize)); no bare
  String/u64 across a crate boundary. Checked or saturating arithmetic on all budget, token,
  and offset math.
- Typed errors (thiserror) at crate boundaries; wire-facing enums carry Other(String).
- A comment earns its line only by naming what the code cannot: the incident that created a
  constant or guard, an invariant the type system cannot express, or a schema fact on a
  yi-types public item. Restating a signature, a name, or the next three statements is none of
  these. The test is content, not sigil: /// is allowed wherever a comment is earned and banned
  where it is not. Three lines per comment, hard — only a license/attribution header may exceed
  it. No narrative comments, no section banners, no commented-out code. check_comments.py backs
  it: the length cap outright, comment volume outside yi-types as a shrink-only ratchet.
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
