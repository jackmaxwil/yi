# The F0c contract fixtures: what the files beside this one say

Plan sections 6.1 to 6.5. This file is to `contracts/` what `../journal/canonical.md`
is to `../journal/`: the rule the fixtures were written against, so a reader that
disagrees with them disagrees with the rule. Nothing here is Rust. The fixtures
are data first on purpose; the verifier is written to satisfy them, not the other
way round.

## The fixture shape

A fixture in this directory is a `plan_walkthrough.rs` fixture plus the keys F0c
needs. The existing keys keep their meaning exactly (`id`, `description`,
`decisions`, `prompt`, `eagerInit`, `goal`, `plan`, `steps`, `final`); the driver
that reads these files is the walkthrough driver with the rows below added.

New top-level keys:

| key | meaning |
|---|---|
| `artifacts` | alias to blob. Each entry carries `digest`, `media_type`, `length` and either `text` (the bytes inline) or `file` (a sibling file in this directory holding them). The driver writes each blob into the plan's `artifacts/` store before step 0 and refuses a fixture whose declared digest does not match the bytes, so an edit to a blob that forgets its digest is a hard failure, not a silent drift. |

New step keys:

| key | meaning |
|---|---|
| `workspace` | relative path to artifact alias: files staged into the verification workspace before the op runs. This is how the red-then-green fixture replaces a product between two verifications. |
| `serve` | url to artifact alias: what the `OutputResolve` stub answers for that url. Absent url, absent answer. |
| `unserved` | urls the stub answers `Ok(None)` for: resolved but unserved, the case `unserved-output-today-passes.json` is about. |
| `whileVerifying` | ops the driver applies after the `verification_requested` record commits and before the token comparison. The only window in which a todo can be retried out from under its own verification. |

New assertion keys inside `expect`:

| key | meaning |
|---|---|
| `verdict` | `outcome`, `score`, `coverage` (both permille), `items`, and the parts of `token` the fixture pins. `items` is a flat list of `{id, verdict, detail?}`: an assertion language, not the wire type, which encodes `ItemVerdict` as `"pass"` or `{"fail": {"detail": ...}}`. A `detail` assertion is a substring test. |
| `cases` | per-case outcomes for an `example` item: `index` (zero based, into `cases.json`), `outcome`, and `why`, the distinct failure reason. |
| `staleToken` | the parts of the refused token the fixture pins, for a refusal that carries a token rather than a verdict. |
| `journal` | the record kinds this op appended, in order. A fixture that expects a refusal and names no journal row is asserting only half of §6.3. |
| `todos[].attempt`, `todos[].refusals`, `todos[].resolution`, `todos[].retries` | the four todo fields F0c reads. `resolution` is `verified_done`, `accepted_by_user` or `legacy_unverified` and is asserted only on a `done` state. |

New assertion keys inside `final`:

| key | meaning |
|---|---|
| `contractDigestUnchanged` | every verdict in the run carried the same `contract_digest`. This is how a fixture says the product was fixed and the contract was not, without writing a digest literal that rots on the next edit. |
| `criteriaDigestUnchanged` | the same for `criteria_digest`. |
| `outputDigestChanged` | at least two verdicts carried different `output_digest` values. Paired with the row above it is the red-then-green claim in full. |

## The checker manifest

A `cmd` or `example` decider names a frozen manifest by `ArtifactRef`, never a
pathname: a criterion that can be edited between two verifications is not a
criterion. `checker-manifest.example.json` is the shape, every field required,
no defaults, unknown keys refused.

| field | type | meaning |
|---|---|---|
| `manifest` | integer | format version, `1`. Bumped, never reinterpreted. |
| `command` | string | the one command, run as `/bin/sh -c` by absolute path with `env_clear()`. Not a list, not a shell function, not a path to a script the manifest does not also protect. |
| `cwd` | string | `snapshot_root` or `snapshot_subdir`. The closed set is the point: a checker does not choose its own working directory, and there is no `absolute` member. |
| `cwd_subdir` | string or null | the path under the snapshot root, relative, no `..`, legal only with `snapshot_subdir` and null otherwise. |
| `protected` | array of paths | paths under the snapshot root the check may not change. Each is digested before and after the item runs; a run that changed one (or created or removed it) fails the item rather than passing it, so a checker script that lives inside the snapshot and rewrites itself mid-run fails. The product's edits before the attempt's snapshot are what the frozen manifest digest already pins: a manifest names its checker by artifact digest, never by pathname. |
| `timeout_ms` | integer | the item's own wall-clock bound, on top of the whole verification's deadline. Milliseconds, said here because every cap in this plan names its unit. |
| `env` | array of names | the variables the command may see, on top of `PATH`, `HOME`, `LANG`, `TMPDIR`. Names only: a manifest never carries a value, so a secret cannot be frozen into a criterion and read back out of the store. |
| `reads_outside_snapshot` | bool | the checker declares that it reads inputs the snapshot does not contain. True does not refuse the item; it marks the verdict non-reproducible, which is what an honest verdict over an unpinned input is. |

The manifest is canonicalized by `../journal/canonical.md`'s rule and hashed by
its bytes as stored, so the digest in a contract and the digest of the blob in
`artifacts/` are the same string.

The examples runner compares a case's answer to `expected` as parsed JSON values:
key order and whitespace do not fail, and numbers compare by their JSON
representation, so `1` and `1.0` differ and there is no tolerance. A case that
needs one writes the tolerance into its own frozen checker, never into the runner.

## The journal fixture

`../journal/verification.jsonl` carries one record of each verification kind this
stage adds, chained by the same rule as `records.jsonl`: `verification_requested`,
`done` with a `verdict` and a `resolution`, `done_refused`, `verification_stale`.
The `verdict` field is already on `JournalRecord`
(`crates/types/src/plan/ledger.rs`), so nothing about the envelope changes.

Envelope keys are camelCase and the keys inside `args` and `verdict` are the
Rust field names, snake_case, the same two conventions meeting in one record that
`records.jsonl` already has.

The records between these four (the `start` before the first, the second
`verification_requested` whose effect `e-v1b2d9` the last two records settle,
the `fail`, `retry` and `start` before the last) are elided, so this is a shape
and chain fixture the reducer never reads, exactly as the `import` record in
`records.jsonl` is. The digests chain; the state does not reduce.
`plan_journal::the_verification_fixture_carries_the_shapes_the_kernel_writes`
chain-verifies it and parses each `verdict` back into `Verdict`, so the file
cannot drift from the bytes `done.rs` writes: `items` is a list of
`{"id", "verdict"}` lines, a `verification_requested` carries the requesting
process's `claim` (`pid`, `at`), and every settling record names its
`effect_id` (`done` and `done_refused` in the envelope, `verification_stale`
in its args). The digests inside the tokens are real:
`contract_digest` is the canonical digest of the contract in
`writer-cmd-red-then-green.json`, `criteria_digest` and the two `output_digest`
values are the digests of that fixture's `checker`, `product-red` and
`product-green` blobs, so a reader can cross-check the whole file against the
fixture beside it.
