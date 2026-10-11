# Disk retention: test homes die with their worktree, Yi sweeps what it writes

```
status:  ACCEPTED, revision 3 (2026-10-07; section 5 changed after a measurement; 2026-10-08
         review rescoped section 5: journeys and dist-suite still write the real ~/.yi).
         Forge issues: #1123 (S2), #1124 (S3), #1125 (S4), #1126 (S5); S1 is tooling.
tree:    main @ 319d0ce5, re-read for this document (ARCHITECTURE 0.525.0, last row D341).
inputs:  du/df census of ~/Development/.yi-homes (116 entries at first look) and ~/.yi on this
         machine · a controlled APFS measurement (sparse volume, shared uv cache, two venvs
         built with Yi's exact uv arguments) · a census of the 11,635 repos under
         ~/.yi/checkpoints · two read-only maps of the tree (stores, dev tooling) · web sources
         on uv, rustup, cargo, APFS and GC precedents (cited inline)
marks:   ✓ exists in the tree, ✚ new
```

## 0. Summary

The owner asked: "investigate why `.yi-homes` accounts for 90 GB. i think we need to look into
better disk usage management with yi".

Most of that 90 GB was never on disk. `du` counts every APFS clone at full size, and every kernel
venv is cloned from the shared uv cache. A second venv built with Yi's own arguments costs
**124 MB** of real disk, all of it precompiled `.pyc`, while `du` reports 370 MB. The real problem
is the one under the miscount: **nothing deletes anything**. The test homes were never removed,
and neither are Yi's own checkpoint repos (11,633 of 11,635 belong to folders that no longer
exist), lane slots, old uv binaries or traces.

The plan has one primitive, a **retention table**. Each row names a store, how to tell that an
entry is still in use (liveness), how old an unused entry must be before it goes (age), and what
to do with it. One sweeper reads the table. It runs in the background at most hourly and on
demand as `yi gc`. Its rows are the cleanups that already exist, which move into it, plus three
new ones.

| Primitive | Shape | Status |
|---|---|---|
| Retention row | `{store, root, live(entry) -> bool, age, action}` | ✚ |
| Sweeper | reads the table; hourly in-process throttle; `yi gc [--fix] [--lane N]` | ✚ (a per-home stamp file; the in-process throttle in `tools/src/spill.rs` `sweep_hourly` stays until the spills row lands) |
| Liveness tests | flock held, `ps` names the path, worktree dir exists, branch merged | ✓ all four exist, in four crates |

The test-home half needs no Yi code. A worktree's test HOME moves inside the worktree at
`target/yi-home`, so `git worktree remove` and `cargo clean` delete it, and nothing has to
garbage-collect `.yi-homes`.

## 1. The problem, measured

### 1.1 `.yi-homes`: duplicated, never deleted, mostly miscounted

At first look `~/Development/.yi-homes` held 116 entries totalling 28 GB by `du` (a concurrent
session was already deleting it, so the 90 GB peak could not be recounted). Of the 28 homes that
had a `.yi`, the subtrees broke down as follows:

| Subtree | `du` total | What it is |
|---|---|---|
| `.yi/kernel-venv-aa885b65` | 9.0 GB (20 venvs, 376 MB each, one hash) | Same venv rebuilt per home, on purpose: the owner's "dont use shared venv" (2026-09-24) |
| `.yi/lanes` | 1.2 GB | Lane worktrees created by test runs inside the home |
| `.rustup` | 1.07 GB in 2 homes | Toolchain copies made when a run forgot `RUSTUP_HOME=$HOME/.rustup` |
| `.cache`, `.uv-cache` | 0.85 GB | uv caches made when a run forgot `UV_CACHE_DIR` |
| top level | ~300 loose files | `*.log`, `*.bak`, `pr*.md`, `redoc_merge.py`, a 1.8 GB trace dir: `.yi-homes` had become a scratch directory |

**The APFS measurement.** On a fresh 20 GB sparse APFS volume, with one `UV_CACHE_DIR` on the
same volume, the venv was built with `uv venv --system-site-packages` and then
`uv pip install --compile-bytecode` of the required set plus the fifteen extras
(`crates/kernel/src/bootstrap.rs:16`, `:537`). Physical usage was read from `df` on that volume:

| Step | Physical | `du` |
|---|---|---|
| CPython 3.11 | 71 MB | |
| venv A, cold cache, bytecode | 398 MB (plus a 268 MB cache) | 370 MB |
| venv B, warm cache, bytecode | **124 MB** | 370 MB |
| venv C, warm cache, no bytecode | **6 MB** | 256 MB |

uv's default link mode on macOS is `clone` (https://docs.astral.sh/uv/reference/settings/#link-mode),
and a clone shares its blocks until either copy is written. `du` counts every clone at full size
(https://eclecticlight.co/2021/04/02/how-can-you-tell-whether-a-file-has-been-cloned-in-apfs/).
The venv's own cost is the `.pyc` files, which are written, not cloned. With the cache shared, as
the per-worktree-home rule already says, a home's venv costs about 124 MB.

Skipping precompilation was measured and **kept** (decision 6). The first import of the whole
extras set in a venv without `.pyc` took 2.03 s against 1.72 s, and lazy compilation wrote only
812 of the 5,016 files (18 MB). D160 chose precompilation because "the first `import pandas` paid
the compile" (docs/archive/plans/2026-09-08-pass-levers.md:156). The 0.3 s is real in benchmark
containers, and once homes die with their worktrees the saving is about 1 GB at ten live
worktrees.

### 1.2 Checkpoints: 2.8 GB, 99.98 % orphaned

`~/.yi/checkpoints` holds **11,635** shadow git dirs totalling 2.8 GB (2,895,296 KiB orphaned). Only **2** have a
`core.worktree` that still exists. 11,554 point under a temp root. Grouped by name prefix, the
largest are `yi-rpc-*` 2,703, `yi-*` 1,942, `yi-acp-*` 1,883, `yi-snap-*` 137 and
`yi-rpc-plan-*` 130. These are test scratch dirs:
`crates/cli/tests/rpc_protocol.rs:14` makes `Scratch::new("yi-rpc-{tag}")` and spawns
`CARGO_BIN_EXE_yi` at `:22` without setting HOME, so every run writes a checkpoint repo into the
developer's real `~/.yi` (memory note "yi tests touch the real HOME").

There is no cleanup at all. Captures are `git add --all` plus `write-tree` with no commit and no
ref (`crates/runtime/src/checkpoint.rs:36-67`), so objects accumulate as loose files forever. The
yi repo's own live checkpoint repo is 394 MiB of 4,736 loose objects. `yi sessions rm` does not
touch checkpoints.

### 1.3 Lanes: warm forever

`Pool::claim` (`crates/runtime/src/lane/mod.rs:712`) creates `~/.yi/lanes/<hash>/<n>` with
`git worktree add` and reuses the slot by path afterwards. Hand-back warms it with a niced
`cargo check` (`toolchain.rs`). The only place a slot dir is ever removed is a slot whose gitdir
vanished (`:741-748`). `yi lanes reap N` (`cli/src/lanes.rs:140`) frees an orphan's claim but
keeps its directory. The pool can grow to 255 slots (D203), each a checkout plus its build
cache, and the 2026-09-04 lanes plan already said orphans "accumulate without bound"
(docs/archive/plans/2026-09-04-lanes-worktree-first-agent.md:289).

### 1.4 Smaller leaks found on the way

| Store | Size here | Cleanup today |
|---|---|---|
| `~/.yi/kernel-venv` (pre-0.111.0 unhashed name) | 261 MB | none: the sweep matches only `kernel-venv-` with a dash (`crates/kernel/src/lock.rs:169`) |
| `~/.yi/uv/<old version>/` | one binary each | none (`crates/kernel/src/uv_install.rs:107`) |
| `~/.yi/traces/<ms>/` | per profile run | none (`crates/cli/src/debug.rs:203`) |
| `~/.yi/sessions/{kernels,family,schedules}/<id>` after ACP `session/delete` | | left behind: ACP calls only `repo.delete` (`crates/acp/src/lib.rs:696-705`) |

## 2. Laws, in the owner's words

- "dont use shared venv" (2026-09-24). Each worktree keeps its own venv. Only the caches under it
  are shared.
- Choices in this session's two rounds, as selected:
  - "Inside the worktree"
  - "Tests + cleanup"
  - "Background + `yi gc` command"
  - "Remove after 14 days unused"
  - "gc absorbs lanes reap"
  - "Trim snapshots older than 30 days"
  - "Keep precompiling"

Two standing laws apply as well. Liveness comes before age: no row deletes on age alone. A row
whose liveness cannot be checked does nothing.

## 3. The retention table

One module, `yi-runtime/src/retention.rs` ✚, owns the table. Following the CLAUDE.md rule
"Multi-axis flows are state-space-as-data", it is data, and no hand-written loop per store sits
beside it. Each row calls a function that lives in the crate that owns the store, so no crate
boundary moves.

| Row | Root | Live when | Age | Action | Status |
|---|---|---|---|---|---|
| spills | `~/.yi/spills/<session>` | `.part` newer than 1 d | kept `.txt` 7 d | delete file, then empty dir | ✓ `tools/src/spill.rs:180` (row wraps it) |
| converted | `~/.yi/converted` | newer than 30 d | 30 d, then size cap 256 MiB | evict oldest | ✓ `tools/src/document.rs:496` |
| kernel venvs | `~/.yi/kernel-venv-*` **and the legacy `kernel-venv`** | flock held, or `ps` names it, or mtime < 24 h | 24 h | delete dir | ✓ `kernel/src/lock.rs:152`, ✚ legacy name |
| kernel connections | `~/.yi/kernel-connections/<pid>-…` | pid alive | none | delete dir | ✓ `kernel/src/connection.rs:104` |
| checkpoints, orphan | `~/.yi/checkpoints/<xxh32>` | `core.worktree` dir exists; kept when it cannot be stat'd or its nearest surviving ancestor is a mount root (unmounted volume) | 7 d since last capture | rename aside, then delete | ✚ |
| checkpoints, live | same, live repos | object mtime < 30 d | 30 d | `git prune --expire=30.days.ago`, never a delete (review: a delete raced a session starting there) | ✚ |
| lane slots | `~/.yi/lanes/<hash>/<n>` | `.held` flock taken, or tree dirty, or branch unmerged | 14 d since release | `git worktree remove`, drop `N.json` | ✚ (liveness ✓ `abandoned` `lane/mod.rs:890`) |
| uv binaries | `~/.yi/uv/<ver>` | `ver == UV_VERSION` | none | delete dir | ✚ |
| traces | `~/.yi/traces/<ms>` | newer than 30 d | 30 d | delete dir | ✚ |

**Why `git prune` is safe for live checkpoints.** No ref points at any captured tree, so every
object is unreachable and `git prune --expire` decides on file mtime alone
(https://git-scm.com/docs/git-prune). Git freshens the mtime of an existing loose object whenever
it would write it again, so a blob or tree that a recent capture reused is younger than the
cutoff even if it was first written months ago. The cost is the decision itself: `undo` to a
checkpoint older than 30 days stops working. Repacking would not shrink anything, because
`git repack` packs only reachable objects and nothing here is reachable.

**Why lanes key on release time.** A slot's directory mtime moves when anything inside it is
touched, the warmer's `cargo check` included. The slot's `N.json` is rewritten at hand-back
(`detach`, `lane/mod.rs:946`). Its mtime is the release time, and the sweep reads that.

### 3.1 Triggers

- **Background.** The first `yi` process in a home to pass the hourly throttle runs the sweep on a
  detached, niced thread, the way `remove_stale_venvs` is spawned today
  (`kernel/src/bootstrap.rs:866`). The throttle is a stamp file `~/.yi/retention.stamp` ✚,
  compared by mtime. The per-process `sweep_hourly` in `tools/src/spill.rs` keeps its job until
  the spills row lands; the stamp already keeps two hundred test processes from each scanning
  the same home.
- **`yi gc`** ✚. It runs every row now, ignoring the throttle, and prints one line per row: kept
  out of found, removed, bytes freed. Bare `yi gc` prints the same lines and deletes nothing; `--fix` acts, as `yi doctor --fix` does.
  `--lane N` force-frees one slot and is the old `yi lanes reap N`. `yi lanes reap` stays as an
  alias for one version, then goes (one-in-one-out, section 7).
- **`yi doctor --fix`.** Its `lanes` row (`cli/src/doctor.rs:233-262`) calls the lane row of the
  table instead of its own loop.

As built (S3), the report carries counts only. A byte figure was tried and cut: apparent bytes
said 984 MB where `du` said 2.8 GB, and neither is what deleting frees on APFS (section 1.1).
The existing sweeps (spills, converted documents, stale venvs, kernel connections) keep their own
triggers and did not move into the table: moving them would buy no behavior. A short-lived
process such as `yi ask` can exit before its background sweep finishes; the sweep is idempotent,
and the stamp is written only once a sweep finishes, so the next session picks up where it
stopped. A sweep in flight holds `~/.yi/retention.flock`, which the OS drops with a killed
holder, so a second session skips it. Known limit: a persistent Linux mount point
(`/mnt/data`) left empty by an unmount reads as a deleted project, and its repos go after 7 days.

## 4. Test homes: inside the worktree

The current rule (memory note `per-worktree-home-not-shared-venv`) is
`env HOME=$HOME/Development/.yi-homes/<wt> CARGO_HOME=$HOME/.cargo RUSTUP_HOME=$HOME/.rustup UV_CACHE_DIR=$HOME/.cache/uv just check`.
It changes in two places.

1. **HOME moves to `<worktree>/target/yi-home`.** `target/` is ignored, so the checkpointer's
   `git add --all` skips it, and `git worktree remove` (which ignores ignored files) and
   `cargo clean` both delete it. Liveness is containment: a home lives exactly as long as its
   worktree. `.yi-homes` is no longer created, and the scratch files that collected there
   (section 1.1) have nowhere to go but the session scratchpad, so item 3 of the original list
   needs no rule of its own.
2. **One wrapper script makes the env** ✚ `scripts/wt_home.sh`:
   `exec env HOME=$wt/target/yi-home CARGO_HOME=$REAL/.cargo RUSTUP_HOME=$REAL/.rustup UV_CACHE_DIR=$REAL/.cache/uv RUSTUP_AUTO_INSTALL=0 "$@"`,
   after copying `~/.gitconfig` in. It spells `$HOME` rather than `~`, which closes the 2026-09-28
   incident where `CARGO_HOME=~/.cargo` made a literal `./~` holding a 1.4 GB toolchain. Subagents
   already need a wrapper because their Bash refuses inline `env HOME=…` (memory note "perf
   program landing lessons").

`RUSTUP_AUTO_INSTALL=0` is there because sharing one `RUSTUP_HOME` is safe for reading installed
toolchains but not for concurrent installs, which can corrupt a toolchain
(https://github.com/rust-lang/rustup/issues/988, open). The pinned 1.94.0 is installed once by
hand, and a worker that would have raced an install fails loudly instead.

The rule for `git push`, `fgj` and `just push` is unchanged. They run with the real HOME
(memory note "session scratchpad can vanish": fgj needs the real HOME).

## 5. Tests stop writing to the real `~/.yi`

Revision 2, after a measurement. One full `cargo nextest run --workspace` on the real HOME (2,710
tests) left 23 checkpoint repos in `~/.yi`, plus writes to `spills/`, `tool-output/`, `mcp/` and
`node/slots/`. Six of the repos came from `rpc_protocol.rs`, which spawns the binary without
`HOME`. The other seventeen came from five in-process runtime test files (`kernel_sandbox`,
`kept_rules`, `snapshot_e2e`, `kernel_across_sessions`, and the family tests). They pass the real
`$HOME` into `RuntimeWiring` on purpose: it shares one warm kernel venv, and it keeps the sandbox
tests' HOME outside the temp roots, where Seatbelt grants would make them pass vacuously.

Patching each site with a scratch HOME would break both of those properties. The suite entries
that route through `just test` (`just check`, `just lane test`, the pre-push hook) are covered
there: `just test` re-executes itself under `scripts/wt_home.sh`. Whatever any test writes to
HOME lands in `<worktree>/target/yi-home`, which is outside the temp roots, holds one shared
venv for the run, and is deleted with the worktree. That makes the per-site helper and its
guardrail unnecessary.

Two suite entries sit outside that seam and still write the real `~/.yi` on every run: `just
journeys` (called by `just postmerge` and the CI postmerge lane) runs `cargo test --workspace
-- --ignored` at the ambient HOME, and `just dist-suite` (CI nightly `dist-suite.yml`) runs
`cargo test --workspace --profile dist` the same way. The tier-2 journeys they run boot real
kernels and spawn the binary at the ambient HOME, so kernel venvs, checkpoints and spills still
land in the developer's `~/.yi`. Routing both through the wrapper is the remaining half of
#1123; this PR closes the `just test` seam only.

The journey that proves it: run the suite through the wrapper, and the count of
`~/.yi/checkpoints/*` before and after is equal.

## 6. Worked example: one PR worktree's life

1. `git worktree add ~/Development/yi-s7 -b jack/x origin/main`.
2. `just check`. `just test` moves HOME to `yi-s7/target/yi-home`. The kernel venv is built there
   from the shared uv cache, at 124 MB physical, and nothing lands in `~/.yi`.
3. The PR merges. `git worktree remove yi-s7` deletes the checkout, `target/` and the home with
   it.
4. Meanwhile, in the developer's real `~/.yi`, the hourly sweep deletes the checkpoint repos of
   the temp dirs that older sessions left behind, once each has been gone seven days. It also
   prunes snapshot objects older than 30 days in live projects and removes lane slots released
   more than 14 days ago. `yi gc` shows what the next sweep would take.

## 7. Inventory

| Piece | Where | Status |
|---|---|---|
| Hourly throttle | `tools/src/spill.rs:180` `sweep_hourly` | per-process guard stays until the spills row lands; the stamp file covers the home sweep |
| Age sweep, link-safe | `tools/src/spill.rs:193-214` | ✓ becomes the spills row |
| Age plus size eviction | `tools/src/document.rs:496` | ✓ becomes the converted row |
| Guarded sibling sweep | `kernel/src/lock.rs:119-169` | ✓ venv row; ✚ legacy name |
| Dead-pid sweep | `kernel/src/connection.rs:104` | ✓ connections row |
| Lane liveness | `lane/mod.rs:883-913` (`repo_gone`, `abandoned`, `reap`) | ✓ |
| Lane slot removal | | ✚ |
| Checkpoint orphan and age rows | `tools/src/checkpoint.rs` | ✚ |
| Retention table and sweeper | `runtime/src/retention.rs` | ✚ |
| `yi gc` | `cli/src/main.rs` subcommand match (`:1048-1148`) | ✚ |
| `yi debug paths` lists every root | `cli/src/debug.rs:83-104` (omits checkpoints, spills, converted, venvs) | ✚ four lines |
| `just test` re-executes under the wrapper | `justfile` `test` recipe | ✚ one line |
| `scripts/wt_home.sh` | | ✚ |

**What it deletes.** `yi lanes reap` becomes an alias for one version, then goes. The `lanes`
loop in `yi doctor --fix` is deleted. Four separate per-store throttles and triggers fold into
one stamp. The `.yi-homes` convention goes, along with the memory rule that created it, which is
rewritten.

## 8. Build order

Each stage is a demo and a gate. The stages that hurt most and need no new infrastructure come
first.

| Stage | Demo | Gate |
|---|---|---|
| S1 HOME inside the worktree (no Yi code: `scripts/wt_home.sh`, the memory rule, a `.ruler` line) | `git worktree remove` frees the home; `.yi-homes` never reappears | one PR landed using it |
| S2 Tests own their HOME (#1123) | `just check` leaves `~/.yi/checkpoints` count unchanged | a full suite run on the real HOME leaves 23 repos (seen red); through `just test` it leaves 0 |
| S3 Retention table, checkpoint rows, legacy venv, `yi gc` (#1124) | `yi gc` on this machine lists about 11,600 orphan repos and 2.8 GB; the real run frees it by `df` | a test per row at age − 1 and at age, plus one with a live holder |
| S4 Lane slot row; `yi lanes reap` becomes `yi gc --lane` (#1125) | a slot released 15 days ago (mtime set) is removed; one held or unmerged is kept | feature-ledger `journey:` test over a real pool |
| S5 uv and traces rows, ACP delete leak (#1126) | `yi gc` reports them | per-row tests |

S1 is not a Yi change and lands as a tooling commit. S2 to S5 each have their own forge issue.

## 9. Decisions log

Every decision, as the owner chose it on 2026-10-07:

| # | Question (short) | Owner's choice |
|---|---|---|
| 1 | Where should a worktree's test HOME live? | "Inside the worktree (Recommended)" |
| 2 | Checkpoints: what should the fix cover? | "Tests + cleanup (Recommended)" |
| 3 | When should Yi run its own cleanup? | "Background + `yi gc` command" |
| 4 | Remove unused lane slots? | "Remove after 14 days unused (Recommended)" |
| 5 | One-in-one-out for `yi gc` | "gc absorbs lanes reap (Recommended)" |
| 6 | Trim old snapshots in live projects? | "Trim snapshots older than 30 days (Recommended)" |
| 7 | Kernel venv bytecode | "Keep precompiling (Recommended)" |

Two items from the original list changed on contact with the tree:

- **"`chmod -R u+w` first" is dropped.** `~/.yi` holds 201,700 read-only files but no directory
  without the owner-write bit, and `~/.rustup` and `~/.cache/uv` hold none either. Deletion needs
  write permission on the parent directory, not on the file (https://keith.github.io/xcode-man-pages/rm.1.html),
  so `rm -rf` succeeds.
- **"Share `RUSTUP_HOME` and `UV_CACHE_DIR`"** was already the rule. The leak was runs that forgot
  it, and `scripts/wt_home.sh` makes forgetting impossible.

## 10. Open questions

- **Worktree `target/` dirs are the real disk.** The concurrent cleanup that ran during this
  investigation took the volume from 19 GiB free to 630 GiB free, and almost none of that was
  `.yi-homes`. An earlier census found 47 dead worktrees holding about 490 GB of `target/` (memory
  note "Mac disk: worktrees + TM snapshots"). `just worktrees` could flag a worktree whose branch
  is merged and whose dir has been idle 14 days. Is that in scope, or its own issue?
- **Does a HOME nested under the repo break anything?** No test is known to assume HOME is outside
  the cwd, and the checkpointer already excludes its own root when the root sits inside the cwd
  (`tools/src/checkpoint.rs:68-71`). Lanes would nest `git worktree add` under `target/`. S1 runs
  the whole suite once this way before the rule changes.
- **Should captures be reachable?** A ref per session (`refs/yi/<session>`) would let `git gc`
  pack checkpoints instead of keeping them loose. Today's census does not show live repos large
  enough to pay for it.
- **Who prunes the shared uv cache (15 GB here) and the seven rustup toolchains (8.7 GB)?** Both
  belong to their tools, not to Yi: `uv cache prune` and `rustup toolchain uninstall` are recipes,
  not rows. uv 0.12.2+ can report physically freed space (https://github.com/astral-sh/uv/pull/20925).
  This machine has 0.12.1.

## 11. D-rows owed

Re-read the ARCHITECTURE version header and the last D-row immediately before writing either, since
the numbers below are the next free ones on 2026-10-07 and may be taken by then.

- **D342 (S3).** Yi's stores are swept by one retention table of `{store, live, age, action}`
  rows, run hourly behind a stamp and on demand by `yi gc`. Liveness gates every row, and age
  never deletes on its own. Checkpoint repos whose worktree has been gone 7 days are deleted, and
  live repos are pruned at 30 days. Reversible via: delete the module; each row's old trigger
  returns.
- **D343 (S4).** A free lane slot released more than 14 days ago is removed. This revises D203's
  "every slot stays warm by path", and `yi lanes reap N` becomes `yi gc --lane N`. Reversible via:
  drop the row.

Each lands with its ADR under docs/solutions/adr/, a changelog row citing its issue, a
feature-ledger row with a `journey:` clause, and a YI_DESIGN §1.2 edit for the demoted verb.

## 12. Not building

- **A shared kernel venv.** It is ruled out by the owner, and APFS clones already make a
  per-worktree venv cost 124 MB.
- **Dropping `--compile-bytecode`.** Decided under decision 7.
- **A size-budget eviction across all of `~/.yi`.** Every row decides by liveness and age. Only the
  converted cache, which already has a size cap, evicts by size.
- **A `just homes-gc` recipe.** It is made unnecessary by decision 1.
- **A config key for retention ages.** The ages are constants until a log shows one is wrong.
  Config is `deny_unknown_fields`, and every key is surface area.
- **Pruning the uv cache, cargo's cache or rustup toolchains from Yi.** Those are other tools'
  stores. Cargo already garbage-collects its own cache automatically since 1.88
  (https://blog.rust-lang.org/2025/06/26/Rust-1.88.0/).
