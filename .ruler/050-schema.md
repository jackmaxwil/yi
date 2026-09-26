# Schema stability (YI_DESIGN.md §20 — load-bearing)

- Every serialized shape (wire and disk) lives in yi-types. No serde derive outside it except
  test fixtures.
- Additive evolution only: new fields Option or defaulted; never rename (serde alias instead)
  and never repurpose. Breaking change = version bump + idempotent migration fn + committed
  before/after fixtures.
- Unknown data survives round-trips: flattened extra maps on durable structs; unknown enum tags
  decode to Other(String) and re-emit verbatim. Never deny_unknown_fields on durable data
  (config is the deliberate exception: strict, failing key named).
- Golden fixtures under yi-types/tests/fixtures/ deserialize forever; a fixture is never
  deleted, only added. A schemas.lock diff is a reviewed artifact.
- Pi's v4 session JSONL is the one external format anchor: the fixtures under
  crates/types/tests/fixtures round-trip byte for byte (YI_DESIGN.md §4.1).
