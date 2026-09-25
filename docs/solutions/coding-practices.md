# Coding practices

The enforced rules are `.ruler/` (instructions) and [YI_DESIGN.md](../YI_DESIGN.md) §18-§21
(gates). This page names where each practice lives rather than restating it.

## Rust

- Code style, panic budget, comment rules and their gates: YI_DESIGN
  [§19](../YI_DESIGN.md#19-code-style-har); the narrative and the incidents behind each rule are in
  `.ruler/030-style.md`.
- Comment referents and grants (`Incident:`, `Invariant:`, intra-doc links):
  [comment-style.md](comment-style.md) has the conversion recipe.
- A cap the model cannot see is a lie about the file: every cap, clamp, page or skip that shrinks
  a model-facing view names itself at the cut and has the test that trips it
  (`.ruler/045-loud-caps.md`, YI_DESIGN [§7.5](../YI_DESIGN.md#75-loud-caps)).
- Every serialized shape lives in `yi-types`, and a shape changes only through
  `scripts/guardrails/baselines/schemas.lock`: YI_DESIGN [§20](../YI_DESIGN.md#20-schema-stability).

## Dependencies

A crate enters only through the YI_DESIGN [§18.3](../YI_DESIGN.md#183-allowed-dependencies)
table, `deny.toml` and a [size-ledger.md](../size-ledger.md) row in one commit (§18.1). Each
`deny.toml` advisory ignore names its removal condition.

## Workflow

- `just check` (lint, guardrails, tests) before claiming done; quote failures verbatim. The gate
  list is YI_DESIGN [§21](../YI_DESIGN.md#21-guardrails).
- Ratchets only shrink; growth is `--update` in a commit of its own, after the code commit.
- A structural change bumps [ARCHITECTURE.md](../ARCHITECTURE.md) and adds a
  [CHANGELOG.md](../CHANGELOG.md) row; revising a settled decision needs a new D-row first; each
  D-row gets its ADR through `just adr <N>`. Commit, PR and tracking rules are
  `.ruler/090-workflow.md`, `.ruler/095-tracking.md` and `.ruler/097-landing.md`.

## Tests

`.ruler/080-testing.md` is the doctrine: each test defends one externally observable contract,
is seen red against the unfixed code, and uses a production-shaped fixture. Session wire fixtures
(`crates/types/tests/fixtures`) are Pi's v4 JSONL, asserted byte-identical; provider tests replay
canned SSE through the mappers with no keys. T0 unit and contract tests and T1 faux cassettes run
in `just check`; T2 real-binary journeys run in `just journeys`; T3 paid runs are user-run and
ledgered in [eval-ledger.md](../eval-ledger.md). Property tests use `proptest` (a dev dependency
of `yi-types` and `yi-runtime`).
