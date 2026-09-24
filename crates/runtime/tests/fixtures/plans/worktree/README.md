# The F0d worktree fixtures: the repository and the records

Plan section 6.6, with section 7.4's repossession and section 7.5's stuck notice
riding along. This file is to `worktree/` what `../contracts/contracts.md` is to
`../contracts/`: the rule the files beside it were written against, so a reader
that disagrees with them disagrees with the rule. Nothing here is Rust. The
acceptance coordinator is written to satisfy these files, not the other way
round.

Two files, one here and one in the journal directory, each read by a test:

| file | what it is | read by |
|---|---|---|
| `repo.sh` | the fixture repository recipe, git and sh only | `lanes.rs`, through `Rig::fixture_repo` |
| `../journal/acceptance.jsonl` | one record of every shape this stage adds, chained by `../journal/canonical.md`'s rule | `plan_journal::the_acceptance_fixture_carries_the_shapes_the_coordinator_writes` |

## The repository

    sh repo.sh <dir> [clean|dirty|staged]

builds `<dir>/parent`, a checkout on `main` with one tracked file, `rotate.sh`,
and leaves four branches in it. It refuses a `<dir>` that exists: a half built
fixture is worse than none. The second argument is `clean` by default; `dirty`,
`staged` and `nested` leave the user's uncommitted work in the parent checkout.

| ref | what it carries | what it is for |
|---|---|---|
| `main` | `list` on line 2, `retention: 14 days` at the end | the parent generation, moved after the candidates were cut |
| `yi/cand-green` | `list rotate`, and a new `docs/ROTATE.md` | passes the frozen checker on its own branch and merges cleanly into the moved generation; the new file is what a ref-only publication must still write into the parent checkout when no edit of the user's overlaps it |
| `yi/cand-red` | `list compress` | fails the frozen checker; it merges cleanly, which is the point, because nothing should ever ask it to |
| `yi/cand-conflict` | `list rotate` and `retention: 30 days` | conflicts with the moved generation on the retention line |
| dirt (`dirty`) | an unstaged edit to `rotate.sh`, an untracked `scratch.txt` | the user's working tree, which an integration must not touch and a `git add -A` anywhere near the parent sweeps into a commit |
| dirt (`staged`) | the same edit to `rotate.sh`, staged; the same untracked `scratch.txt` | the user's index, which a ref-only publication must leave as it was: their hunk stays staged, and nothing of the integration shows as a staged reversal |
| dirt (`nested`) | an untracked `docs/` holding the user's own `docs/ROTATE.md` | the path the green candidate adds, inside a directory `git status` collapses to `docs/`: a ref-only publication must leave the user's bytes there, not restore the integration's over them |

Three filler lines sit between the two lines a candidate and the generation
write. That spacing is load bearing: git merges by hunk with three lines of
context, so two edits any closer together conflict on their adjacency alone and
the clean candidate would fail for the wrong reason. A recipe that tightens
`rotate.sh` turns `changed_parent_generation_rejects_stale_integration` green
for free and makes `failing_candidate_never_contaminates_parent_checkout` prove
nothing.

The recipe does not pin commit dates, so shas differ between builds. Every
assertion over them is symbolic, and no fixture writes a sha literal.

### The helper `lanes.rs` adds

```rust
/// Build the section 6.6 fixture repository under this rig's scratch and return
/// the parent checkout. `dirt` leaves the user's uncommitted work in it.
impl Rig {
    fn fixture_repo(&self, dirt: Dirt) -> Result<PathBuf, Box<dyn Error>>;
}
```

It runs `repo.sh` with `yi_tools::command("sh")` against
`Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plans/worktree")`,
into a fresh subdirectory of the existing `Rig`'s `Scratch`, and returns
`<dir>/parent`. It does not replace `Rig::new`'s own one commit repository,
which the claim and pool tests still want: a test that needs candidates calls
this, and every other lane test is untouched. `Dirt` is `Clean | Dirty |
Staged`, because a bool argument at a call site reads as neither.

## The journal fixture

`../journal/acceptance.jsonl` carries one record of every kind this stage adds,
chained by `../journal/canonical.md`'s rule from an empty root, and canonical
byte for byte. Five records walk one todo from a submitted candidate to an
accepted integration; four more carry one disposition each. The records between
them (`start`, the two `verification_requested`) are elided, so this is a shape
and chain fixture the reducer never reads, exactly as `records.jsonl`'s `import`
record is: the digests chain, the state does not reduce.

| seq | `op` | what it says |
|---|---|---|
| 1 | `candidate_submitted` | the child is quiescent (`quiescent.running_commands` is 0), its work is a commit on `yi/cand-green`, and its outputs are named before anything else happens |
| 2 | `candidate_verified` | the candidate tree passed on its own: the token's `snapshot` is the candidate commit and its `integration` is null |
| 3 | `integration_prepared` | the generation picked, the staging worktree, the merged tree, and `conflict_provenance`, all under the integration lock |
| 4 | `integration_verified` | the same frozen checker on the merged tree: the same `contract_digest` and `criteria_digest`, a different `snapshot`, `integration` 1 |
| 5 | `accepted` | `from` running `to` done with `resolution` `verified_done`, carrying both tokens and the published commit. The only record here that moves a todo |
| 6 to 9 | `disposition` | one of each member, below. No `to`, so no row in `STEPS`, as `done_refused` has none |

`Disposition` is externally tagged snake_case, so a record reads
`{"disposition":{"merge_failed":{...}}}`, the way `ItemVerdict` reads
`{"fail":{...}}`.

| member | fields | when |
|---|---|---|
| `retained` | `branch`, `candidate`, `kept`, `reason` | the work stays on its branch; the branch is the copy, so the slot is released and nothing is merged |
| `discarded` | `branch`, `candidate`, `kept`, `reason` | the branch is deleted, so `kept` is written first and holds the only copy |
| `merge_failed` | `branch`, `candidate`, `parent_base`, `generation`, `conflict_provenance`, `retry_id` | the prepared merge conflicted; retained by construction and retryable by `retry_id` |
| `repossession_pending` | `branch`, `candidate`, `kept`, `lease`, `at`, `detail` | F2b writes this one: stopping, settling or committing failed, `slot_released` is false, and nothing is released while a process can still write |

`conflict_provenance` is a list, empty on a clean merge, of
`{path, side, base}`: every path the prepared merge had to choose at, which side
won, and the merge base it chose against. It is the same shape on the acceptance
record and inside `merge_failed`, so a conflict that was resolved and one that
was not are read by the same code.

The digests inside the tokens are real: `contract_digest` is the canonical
digest of a writer contract with one cmd item over the frozen checker,
`criteria_digest` is the canonical digest of the one-element list of that
contract's frozen artifact digests (`Contract::criteria_digest`, not the
artifact digest itself, which is what `verification.jsonl` wrote), and
`output_digest` is the digest of the candidate's product blob. The test that
reads the file verifies the chain and round-trips every token, verdict and
disposition through the wire types.
