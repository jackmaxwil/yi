# Schema stability (YI_DESIGN.md §19 — load-bearing)

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
- Pi session files are an external anchor: byte-compatible JSONL, conformance fixtures shared
  with Pi's own tests.
