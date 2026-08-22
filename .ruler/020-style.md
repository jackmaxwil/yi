# Style: HAR compliance

The har* skills bundled with this repo are normative for every Rust line: har, har-api,
har-supply, har-verify always; the rest by their stated triggers. Enforced highlights:

- #![forbid(unsafe_code)] in every crate; panic budget zero from day one (clippy denies
  unwrap/expect/panic/todo/unimplemented; check_panic.py backs it).
- Newtypes for every id and unit (EntryId, SessionId, Tokens(u64), Bytes(usize)); no bare
  String/u64 across a crate boundary. Checked or saturating arithmetic on all budget, token,
  and offset math.
- Typed errors (thiserror) at crate boundaries; wire-facing enums carry Other(String).
- No comments except three granted exceptions: incident comments on constants and guards,
  invariant comments the type system cannot express, doc comments on yi-types public items.
  No narrative comments, no section banners, no commented-out code.
- Fight for every line: the size ratchet is a ceiling, not a target.
- Multi-axis flows are state-space-as-data: a table the runtime reads, closed vocabulary,
  invariant checks — never a shadow model maintained beside a hand-rolled flow.
