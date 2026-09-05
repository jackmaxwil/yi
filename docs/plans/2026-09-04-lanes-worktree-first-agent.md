# Lanes: a worktree-first agent that only lands through pull requests

Status: implemented on this branch as D119–D124 (0.146.0). D-numbers below were provisional
(D116+); check collisions against open PRs before any of them is claimed.

## The one-line design

**A yi session never runs in the trunk checkout.** It runs in a *lane*: a
pooled, pre-warmed git worktree on a branch off `origin/main`, and the only way
work leaves a lane is a pull request that the forge merges. Subagent children
are lanes too, branched off the parent's branch and merged back into it. The
trunk checkout is a read-only reference that nobody, human or model, edits.

Everything else in this document is the consequence of that sentence, plus the
levers that make a lane cost nothing to create, warm to build in, and trivial
to throw away.

## What already exists (build on, do not rebuild)

| piece | where | reuse |
|---|---|---|
| `Isolation::Worktree` for children: `git worktree add -b yi/<id>`, `merge` / `discard` hand-back, refuses while running | `crates/runtime/src/worktree.rs` (B11) | becomes the root session's default, not a child option |
| turn checkpoints in a shadow gitdir, `/undo` | `crates/runtime/src/checkpoint.rs` | unchanged; the lane branch holds only curated commits, checkpoints hold turns |
| landing verbs that run the gate first: `just ratchet / commit / push / pr open / pr status / pr merge / land` | `scripts/forge_pr.py`, `.ruler/skills/yi-forge` | the `land` step shells to these; nothing is reimplemented |
| Seatbelt sandbox scoped to "the worktree plus the scratch a build needs" | `crates/tools/src/sandbox.rs` | writable set gains the lane's cache dirs |
| status row already carries `branch`; HUD carries goal + steering | `crates/tui/src/status.rs`, `hud.rs` | gain lane glyphs and a landing row |
| daemon, worker-per-root, heartbeat lanes (H7) | `yi serve`, `runtime::schedule` | PR gate polling is a heartbeat; worktree claim is a worker verb |
| forge as the register of work: `Closes #N`, sizes as labels | D106 | every lane carries an issue number from birth |
| environment block hash-pinned into the prompt | `runtime::environment`, ext slot table | the lane header is one more line in it |

Nothing here needs a new store. The registry of lanes is git itself:
`git worktree list --porcelain` plus the `yi/` branch prefix plus
`git worktree lock --reason session:<id>`.

## Levers, ranked by what they buy

1. **Stable slot paths.** A lane is not a fresh directory; it is one of N
   fixed slots (`~/.yi/lanes/<repo-hash>/<n>`). A build cache is keyed by
   absolute path for every toolchain that matters (cargo's metadata hash
   includes the workspace path; `.tsbuildinfo` and `.venv` are path-bound), so
   a fresh path means a cold cache and a fixed path means a warm one. This one
   decision is worth more than every other optimization combined.
2. **Idle warmers.** A released slot is reset to `origin/main` and its
   toolchain's cheapest whole-tree command (`cargo check`, `tsc --build`,
   `uv sync --frozen`) runs at `nice 19` while the slot waits. The first build
   after a claim then compiles only the diff between the warm base and the
   branch's work, which is what a human's incremental build costs.
3. **Claim is a reset, not a checkout.** `git reset --hard origin/main` on a
   slot whose tree is already at last week's `main` rewrites only the files
   that changed. On yi (about a thousand files) that is tens of milliseconds;
   a cold `git worktree add` is hundreds.
4. **One fetch per repo per minute.** The daemon owns the fetch. A claim
   reuses it when it is fresh, so a session start never waits on the network
   unless the last fetch is older than the threshold.
5. **Shared content-addressed package caches** (`~/.cargo/registry`, the pnpm
   store, `UV_CACHE_DIR`). These are already global; the lane only has to not
   break them by setting `CARGO_HOME` or `HOME` to something private.
6. **Branch = hand-back unit.** A child's merge into the parent branch is a
   local `git merge --no-ff` (exists today). The root's hand-back is push +
   PR. Neither ever touches the trunk checkout, which is how the class of
   accident recorded in memory ("git reset ate a sibling session's edits")
   stops being possible.
7. **Deterministic gate feedback.** PR check state is polled by the schedule
   module and lands as an event, not as a model deciding to look. Red steers
   the session with the job name; green surfaces a merge affordance. No
   LLM-judged firing, in line with the standing rule.

## Per-language surface: one table, not a plugin system

The lane knows a toolchain by a lockfile. Four columns cover Rust, the three
node package managers, and uv; anything else gets the empty row (no sync, no
warm) and still works, just cold.

| detect | sync on claim (fast when unchanged) | warm at idle | env the tool runner sets |
|---|---|---|---|
| `Cargo.lock` | nothing (the warmer did it) | `cargo check --all-targets` | `CARGO_TARGET_DIR=~/.yi/lanes/<repo>/<n>/target` (per slot; a shared dir would serialize sibling builds on cargo's directory lock) |
| `pnpm-lock.yaml` | `pnpm install --frozen-lockfile --offline` | `pnpm run -s typecheck` if present, else `tsc --build` | none: the store is global |
| `bun.lock` / `bun.lockb` | `bun install --frozen-lockfile` | same | none |
| `package-lock.json` | `npm ci` only when the lockfile hash moved, else skip | same | none |
| `uv.lock` | `uv sync --frozen` | `uv run python -c pass` (builds the venv) | `UV_CACHE_DIR` shared; venv per slot, named by lockfile hash (the kernel-venv pattern) |

The lockfile hash is recorded beside the slot after each sync. When it matches
on claim, the sync is skipped outright, so a claim on an unchanged toolchain is
git-only.

Everything outside this table (linker choice, `profile.dev.debug = 0`,
`sccache`) belongs to the repo, not to yi. The table's job is to avoid paying
the cold cost twice, not to make the repo's build faster.

## Architectural decisions (provisional)

- **D116, worktree-first.** `AgentSession` for a root session is created on a
  lane by default. `--here` opts out for the one case that needs the trunk
  (editing the trunk's own git state), and the environment block says so
  loudly when it is on.
- **D119, the pool.** N slots per repo (default 3), fixed paths outside the
  checkout so the trunk's grep and `grid survey` never see them. Claim, release,
  warm, reap are four worker verbs. Slots are created lazily; the pool is never
  larger than the number of lanes ever used at once.
- **D120, git is the registry.** Branch `yi/<session-id>`, worktree locked
  with `session:<id>` while live. A slot whose lock names a session that no
  longer exists on disk is an orphan, and `yi lanes` lists it; reaping deletes
  the branch only when it is merged or the session was discarded, never on a
  timer alone.
- **D121, the hand-back ladder.** Child lane merges into the parent's branch
  (B11 unchanged). Root lane pushes and opens a PR through the existing
  `scripts/forge_pr.py` verbs. Only the forge merges to `main`. `git merge` into
  the trunk checkout is removed from the code path entirely.
- **D122, the forge adapter is detection, not abstraction.** The origin URL
  picks `tea` (Forgejo) or `gh` (GitHub). The Forgejo path is the existing
  script; the GitHub path is the same script with the three API calls swapped.
  No trait; two functions behind one match.
- **D123, gate state is an event.** `Event::LandingState { pr, jobs, behind }`
  emitted by a heartbeat that polls the forge while a PR is open. The TUI
  renders it; a red job steers the session once (latched on the job name, the
  D114 evidence latch pattern) rather than every poll.

## Flow

```
yi [prompt]                         # in any checkout of the repo
 ├─ worker(root) fetches origin/main if the last fetch is > 60 s old
 ├─ claim slot: reset --hard origin/main · branch yi/<sid> · lock session:<sid>
 ├─ sync toolchain if the lockfile hash moved (else nothing)
 ├─ environment block: "lane 2 · yi/<sid> off main@<sha> · land with /land"
 └─ TUI attaches                    # < 100 ms after the daemon is up

work
 ├─ turns checkpoint as today; the branch stays clean until commit
 ├─ children spawn as lanes off yi/<sid>, merge back with the B11 verbs
 └─ heartbeat: main moved? → HUD shows "main +N"; no action until asked

/land "Title" [--closes N]          # or the goal's completion, when autonomous
 ├─ just ratchet · just commit · just push (background, pre-push lane)
 ├─ just pr open  → Event::LandingState{pr, queued}
 ├─ poll → red job → one steering prompt naming the job → fix → push
 └─ green → /land again merges (or auto when the goal allows) → release slot

release
 ├─ unlock · branch deleted when merged · slot marked idle
 └─ warmer: reset to origin/main, run the warm command at nice 19

crash / quit
 └─ lock stays; `yi --continue` re-claims the same slot by session id;
    quit line prints: resume command · lane path · branch · PR
```

## In the TUI

Status row (one line, existing fields kept, lane fields added after `branch`):

```
 opus · high · auto  ~/…/yi  ⎇ yi/s-3e1e ↑3 ↓0  PR #191 ●● ⟳  main +4   2 children   41k/200k
```

- `↑3 ↓0`: commits ahead of / behind `origin/main`.
- `PR #191` with one glyph per gate job in job order (`●` green, `○` queued,
  `⟳` running, `✗` red); no PR yet renders as `unlanded`.
- `main +4`: the base moved under the branch; dim until it matters (a red
  "behind" state from the forge turns it red).

HUD gains a landing row under the goal header, only while a PR is open or a
land is in flight:

```
 Goal close #171 · running · 41k/200k
 ├─ Land · PR #191 · lint ● guardrails ● test ⟳ · 3 commits · main +4
 ├─ Steering · 1
 │    1. gate red: test — cli_surfaces::… timed out
```

Slash verbs, all routed through `yi_runtime::slash` so the console gets them
for free (D112):

| verb | does |
|---|---|
| `/land "Title" [--closes N]` | ratchet, commit, push, PR; the same call on a green PR merges |
| `/pr` | the `just pr status` text in a cell |
| `/base` | `just pr update` (merge `main` into the branch on the forge, then pull) |
| `/lanes` | every slot: idle / warm / held by which session / orphaned |
| `/discard` | release the lane, delete the branch, keep the session transcript |

The land step renders as its own cell, the way the kernel call does (U38): one
row per step with its check, and the forge's refusal verbatim when it refuses.
The session tree (esc-esc) draws child lanes as branches off the parent's
branch, which is what they are.

## Target performance

| operation | target | today |
|---|---|---|
| claim a warm slot (git only, lockfile unchanged) | < 50 ms | `git worktree add` on yi: ~300 ms plus a cold cache |
| claim a cold slot (first use) | < 2 s on yi, < 10 s on a 50k-file tree | same |
| first `cargo build` after claim, Rust | ≈ the human's incremental build (< 30 s on yi) | 3–5 min cold `target/` |
| first `pnpm install` after claim | < 3 s (hardlinks from the store) | 20–60 s cold |
| first `uv sync` after claim | < 1 s | 5–20 s cold |
| TUI attach with the daemon warm | < 100 ms | same |
| `/land` to PR open, gate excluded | < 5 s (push is backgrounded) | manual |
| resident cost per idle slot | zero processes; disk = checkout + its build cache | — |
| resident cost per live lane | the worker it already had | — |

These are targets to measure against, not claims; the first PR under this
proposal should add the two timings that matter (claim, first build) to the
telemetry extension so the ratchet sees them.

## Optimization areas, in the order to take them

1. Stable slots and the git-only claim (D119). Measurable alone.
2. Idle warmers for the three toolchains in the table. Measurable alone.
3. The daemon-owned fetch with a freshness window.
4. Gate polling as an event and the HUD row (D123). Pure UX; no perf.
5. `git maintenance run --auto` in the worker's idle time, and `core.untrackedCache` set per worktree (never through repo-level `git config`, which reconfigures every sibling session at once).
6. Later, only if measured: `sccache` across slots for Rust; `cp -c` (APFS clonefile) of `node_modules` from a warm slot instead of `pnpm install`.

## Ergonomics

For the person:

- `yi` in the repo root starts on a lane and prints one line saying where.
  No question, no flag. The trunk checkout stays exactly as the person left
  it, so their editor never sees agent churn.
- The lane path is a real directory the person can `cd` into, open in an
  editor, or run tests in beside the agent. Their edits are on the same branch.
- The quit line, the session list and `/lanes` all say the same three things:
  path, branch, PR.
- A discarded lane is a deleted branch and an intact transcript, so "throw it
  away" costs nothing and "what did it try" is still answerable.

For the model:

- The environment block states the lane, the base sha, the issue number and
  the one verb that lands work. There is no way to land except through it.
- The `land` step is a tool with structured results (each `just` verb's pass
  or fail and the forge's refusal text), not a bash incantation the model has
  to get right. Permission classifies it as outward-facing: `push` and PR
  creation ask in confirm mode and are allowed in auto mode only when the goal
  says autonomous.
- Red gates arrive as steering with the failing job's name, once per job, so
  the model reads the failure instead of polling for it.
- Children get the same lane the root got, off the parent's branch, with the
  same hand-back verbs it already knows.

## What this proposal deliberately does not do

- No second registry file for lanes. Git's worktree list and lock reasons are
  enough, and a file would drift from them.
- No rebase, ever, on a pushed branch. `/base` merges `main` in; the memory on
  why is the incident record.
- No worktrees inside the checkout. The `.claude/worktrees` layout that Claude
  Code uses pollutes trunk-side tooling; yi's live outside.
- No per-language plugin interface. One table with four rows, extended by
  adding a row.
- No cross-slot shared `target/` for Rust until cargo's directory lock is
  measured to be cheaper than the disk it saves.

## Open questions

- Should `main` in a repo without a forge (no `origin`, or a bare local
  remote) fall back to a local `git merge --no-ff` into the trunk branch? The
  lazy answer is yes, gated on the absence of a remote, and it is the only case
  that ever writes to the trunk.
- Pool size default: 3 slots covers a root plus two children on a laptop.
  Whether the number should track CPU count is a measurement question.
- Whether a child lane should reuse its parent's warm build cache by symlink
  (fast, but two cargo processes on one `target/` serialize) or take its own
  slot's (slow first build, no contention). Start with its own slot.

## HAR review (2026-09-04)

Reviewed against `har`, `har-threat`, `har-async`, `har-concurrent` and
`har-layout`. Findings are ordered by risk; each names the rule, the gap in the
proposal above, and the change that closes it. The decisions D116–D123 are
amended in place below rather than renumbered.

### Findings

| # | rule | gap | change |
|---|---|---|---|
| 1 | threat: draw the boundary; a control after the parser is a control an alternate path skips | The idle warmer runs repo-defined code (`build.rs`, proc macros, `pnpm run typecheck`, uv build backends) from a freshly fetched `origin/main` with no session, no permission verdict, no sandbox, no ledger row. That is elevation (E) and repudiation (R) on the one path the proposal added. | The warmer runs under the same Seatbelt profile as a contained bash call (writable = the slot and its cache dirs, credential dirs denied, no network), always `--offline --frozen`, and **only for a lockfile hash a session has already built** under a permission verdict. An unseen hash leaves the slot cold. Every warm run writes an action-ledger row (D81). |
| 2 | threat: enumerate every input the process did not produce | The proposal lists no inputs. They are: the model's `land` arguments (title, `--closes`, `--refs`); forge API responses (PR number, job names, job states); the origin URL; lock reasons read back from `git worktree list`; the lockfiles; repo-local build config the warmer executes. | Add the boundary table to `docs/ARCHITECTURE.md` beside the crate table, with the six STRIDE letters walked per row, including the dismissed ones. |
| 3 | threat: storage is not sanitization; bound every wire string | Forge job names and refusal text flow into `Event::LandingState`, the HUD, and a steering prompt. A job name is attacker-shaped text (anyone who can push a workflow file) entering the model's queue, and the latch key is unbounded. | `LandingState` carries a parsed `enum JobState { Queued, Running, Green, Red }` plus a job name bounded to 64 bytes at the parser; steering quotes the name inside a fence and the latch keys on the bounded name. Raw `tea api` JSON never leaves the adapter. |
| 4 | har: make invalid states unrepresentable; no `bool` + `Option` for one thing | Slot state is prose ("idle / warm / held / orphaned"); landing state is a PR number plus glyphs plus `unlanded`. Both admit nonsense (held and orphaned, glyphs with no PR). | `enum Slot { Idle { base: Sha, warmed: Option<LockfileHash> }, Held { session: SessionId, branch: BranchName }, Orphan { session: SessionId } }` and `enum Landing { Unlanded, Pushed { branch }, Open { pr: PrNumber, jobs: Vec<Job> }, Merged { pr } }`. Newtypes for `SlotIndex(u8)`, `PrNumber(u32)`, `LockfileHash([u8; 32])`, `Sha`, `BranchName`. `--closes N` is a `PrNumber` parsed with `u32::try_from`, never a string spliced into `Closes #N`. |
| 5 | har: typestate; single-use is enforced by move | `take_settled_worktree` guards "never handed back twice" with a comment and a runtime `Running` check. The hand-back ladder adds a third verb (`land`) on the same shape. | `Lane<Held>` → `settle(self) -> Result<Lane<Settled>, LaneError>` → `merge(self)` / `discard(self)` / `land(self)`. Consumed by move, so a second hand-back does not compile. Marker is `PhantomData<fn() -> S>`. |
| 6 | har: one error variant per distinct caller reaction, carrying the value and the bound | `worktree.rs` returns `Result<_, String>`. The TUI and permission layer need to branch on busy vs orphan vs stale base, which a rendered string cannot offer. | `#[non_exhaustive] enum LaneError { SlotBusy { slot: SlotIndex, session: SessionId }, Orphan { slot, session }, BaseStale { age_ms: u64, max_ms: u64 }, LockfileUnseen { hash }, Git { args: Vec<String>, exit_code: Option<i32> } }`. Stderr text goes in a `source`, never in the variant name. |
| 7 | concurrent: one owner per resource; an advisory lock is a crash record, not a mutex | "Git is the registry" makes `git worktree lock` the claim primitive. It is check-then-write, not atomic; a stale daemon and a fresh one both racing `reset --hard` on one slot is data loss of the held session's tree. | The worker's in-memory `Mutex<Pool>` is the only claim path (the proposal already says "worker verb"; make it the invariant). A process without the worker cannot claim; `--here` is the only non-worker start. The git lock exists so the next daemon can tell held from orphan after a crash. |
| 8 | async: every peer-controlled wait has a timeout; two-phase shutdown; cancel safety | A claim arriving while the warmer is mid-run is unspecified. A killed `pnpm install` leaves a half-written `node_modules`; cargo survives a kill, pnpm does not. The forge poll has no stated timeout. | Claim on a warming slot: signal, wait for exit with a bound (30 s, the existing capture deadline shape), then reset. A slot whose warmer did not exit cleanly records `warmed: None`, so sync-on-claim reruns. Forge poll and `tea` calls take the 120 s deadline `worktree::capture` already applies to git, and their output rides `OUTPUT_CAP`. |
| 9 | threat: resource bounds; "grows with input" vs "bounded" | Orphans accumulate without bound (reaping refuses on a timer alone, correctly), and warmer failures could retry every idle tick. | Pool is bounded at N slots total including orphans; the N+1th claim refuses with `LaneError::SlotBusy` naming the orphans, so `yi lanes` is the fix. A failed warm is recorded against its base sha and not retried until `origin/main` moves. |
| 10 | threat: canonicalize, then check the prefix; secrets never enter an error, an event or argv | Slot paths hash the repo root as given; a symlinked checkout yields two pools. Sandbox writable roots are added as literals. `git push` and `tea` output rendered in a cell can carry a remote URL with inline credentials. | Canonicalize the repo root before hashing and every writable root before the Seatbelt prefix. Route cell text through the E2 inline-credential redaction the proxy path already has. The forge token stays in `tea`'s config and is never read by yi. |
| 11 | threat: model-controlled argv | `tea pr create --title <title>` passes the title as a separate argv item; a title starting with `-` is parsed as a flag by the CLI. This is the model's one direct write to argv. | `--title=<title>` and `--description-file` in `scripts/forge_pr.py`; `BranchName` validated at construction against the ref-format charset (`[A-Za-z0-9._/-]`, no `..`, no leading `-`). |
| 12 | threat: state the failure posture once per component | Not stated. The dangerous default is a claim failure silently falling back to the trunk checkout, which reintroduces the accident class the design exists to remove. | The lane subsystem is defensive (the daemon stays up) but a **claim failure fails the session start** with the `LaneError` printed; there is no fallback to the trunk. Warm failure degrades to a cold slot. |
| 13 | threat: test the control, not the feature | The proposal's tests are timings. | One journey per control, each red when the control is deleted: a root session's cwd is never the trunk; a warmer refuses an unseen hash; an orphan reap refuses an unmerged branch; a `LandingState` from a fixture with a 4 KiB job name is bounded; a `compile_fail` doctest on a second `merge`. |
| 14 | layout: a module is free, a crate is not; a trait with one implementation is a costume | Correct as proposed, recorded here so it is not "improved" later: `runtime::lane` as a module absorbing `worktree.rs`; toolchains as a `const` table; the forge adapter as two functions behind one `match`. No `LaneManager`, no `Toolchain` trait, no new crate. |
| 15 | har: integers | `↑3 ↓0` and `main +4` come from `git rev-list --count`; parse with `u32::from_str`, never `as`, and render saturating. Minor; listed so it is not skipped. |

### Amended decisions

- **D119** gains: the pool is bounded at N including orphans; claim is a worker verb and the only claim path; canonical repo root keys the pool.
- **D120** is reworded: git's lock is the crash record; the worker's mutex is the registry of truth while it runs.
- **D121** gains the typestate ladder (`Held → Settled → merged | discarded | landed`, by move) and the no-trunk-fallback posture.
- **D123** gains the parser bound on job names and the fence-plus-latch rule for steering text.
- **New, D124 (provisional):** the idle warmer is a contained, offline, ledgered run that only rebuilds a lockfile hash a session already built. This is the one decision that did not exist before the review, because the warmer was the one new boundary.

### Not findings

- `Result<_, String>` across the rest of `yi-runtime` is a house convention, not a defect this proposal owns; the new module uses the enum because its callers branch, and nothing else changes.
- The `children` mutex `map_err(poisoned)` pattern in `subagent.rs` already meets the panic budget and carries over.
- Timing side channels, AEAD and nonce rules have no plausible instance at these boundaries; noted as walked, not skipped.
