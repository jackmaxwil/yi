# Yi — governance (campaign 4): growth, tiers, forge, commits, comments

```
status:  SPEC 2026-08-29 — the user-approved five-directive governance package,
         drafted against ARCHITECTURE 0.79.0 / D82 (both re-read from the live
         header immediately before this was written; a parallel reconcile agent
         may claim numbers first, so every D number below is a PLACEHOLDER —
         claim at integrate against a freshly read header, never from here).
         Builders S3–S8 execute §8's matrix; the docs commit lands at integrate.
date:    2026-08-29
sources: docs/ARCHITECTURE.md (header, changelog 0.42.0/0.59.0/0.60.0/0.61.0,
         feature ledger, decision tail D77–D82) · docs/TODOS.md (§O esp. O1/O6,
         §J, §G, K1) · docs/FORGEJO.md · .ruler/{010-done-bar,030-style,
         040-guardrails,080-testing,090-workflow,100-never}.md + ruler.toml ·
         scripts/guardrails/{check_comments.py,check_trailers.py,
         check_guardrails.sh,_common.py} + baselines/ · justfile ·
         docs/plans/2026-08-29-prompt-flywheel.md (laws 1–5, §8 trailers) ·
         git remote (github.com/jackmaxwil/yi) + merged PRs #9/#11 ·
         measured: 52,150 src LOC (crates/*/src/**/*.rs) at 0.79.0; the user's
         17-bump growth study (organic median +35, landings +643..+3295)
```

## 0. Conflicts with the tree (recorded; the tree wins)

1. **The deletion row is 0.60.0, not 0.59.0.** docs/FORGEJO.md:5 cites "the
   failure `.github/` was deleted to avoid (0.59.0)"; the changelog's own
   deletion sentence lives in the 0.60.0 row ("GitHub Actions is deleted
   wholesale rather than carried as a second unrun copy of the rules"). The
   brief inherited FORGEJO.md's citation. Everything below cites 0.60.0; the
   FORGE.md rewrite (§3) corrects the drift.
2. **"No trailers" collides with the flywheel plan's §8.** The approved commit
   standard says no trailers; the landed plan (and TODOS `J11`) defines the
   closed `Opt-Run`/`Opt-Delta`/`Opt-Lever`/`Opt-Cases` trailer set for
   optimizer commits. Tree wins: the standard reads "no trailers, except the
   plan-§8 `Opt-*` set on optimizer commits and the trailers git itself writes
   (`Revert`/cherry-pick provenance). Assistant co-author trailers stay banned"
   (check_trailers.py already gates those).
3. **TODOS `K1` says "the forge is moving to Forgejo."** The forge reality is
   GitHub with live merged PRs; the feature-ledger row for the board already
   says "GitHub issues via `gh`". `K1`'s note is corrected in §9.
4. **Directive 3's branch-protection doctrine already exists** in FORGEJO.md's
   "Branch protection" section, written for Forgejo's settings screen. The
   restoration reuses that text against GitHub's; nothing is invented.

## 1. Directive 1 — growth budgeting (D-row `Dα`)

The size ratchets bound files, crates, comments and the binary, but nothing
prices the repository's total slope, so growth arrives unexamined and is
rationalized after the fact. Grounding: across 17 measured version bumps,
organic changes cluster at a median of **+35 net src LOC** while feature
landings span **+643..+3295** — two regimes, so one flat cap would either
strangle landings or wave everything through. The budget separates them:

- **≤ +150 net src LOC per version** — free. Covers every organic bump ever
  measured with 4× headroom.
- **> +150** — the version's changelog row must carry a **growth memo**: a
  `growth:` clause naming the measured number and what deletion was weighed
  (the deletion-first clause, §7). A landing that cannot say why its bytes
  earn their place does not land.
- **> +2000** — additionally requires a D-row claimed by the same landing:
  growth at that scale is a structural decision by definition.

**The gate** — `scripts/guardrails/check_growth.py`, wired into
check_guardrails.sh beside the other ratchets:

- `baselines/src_loc.json` holds `{"version": "<ver>", "loc": <n>}`
  (seed: `{"version": "0.79.0", "loc": 52150}`).
- Each run measures `sum(len(lines))` over `_common.src_files()` — the same
  file set every size ratchet reads, all lines, src/ only — and reads the
  live header version.
- `delta = measured - baseline.loc`. `delta ≤ 150`: ok. Over 150: the
  changelog row of the *current header version* must match `growth:`, else
  FAIL naming the delta and the missing memo. Over 2000: that row must also
  cite a `D\d+`, else FAIL. Growth over 150 while the header version still
  equals the baseline's version FAILS too — the free band is per version,
  and crossing it is what obliges the bump that carries the memo.
- `--update` rewrites `{version, loc}` and prints `src LOC X -> Y`, in its
  own commit, after the code commit, exactly like every other baseline
  (040-guardrails law verbatim; no exception is created here).

Not a shrink-only ratchet and deliberately so: the budget prices growth
rather than forbidding it, and the memo is the price. Red-first proof at
land: doctor the baseline `loc` down by 151 against a memo-less row and
quote the FAIL verbatim; doctor it down by 2001 against a D-row-less memo
row and quote that FAIL; restore, watch green.

## 2. Directive 2 — E2E tier taxonomy and the journey inventory (D-row `Dγ`)

Four tiers, each named by where it runs and what it may spend:

| tier | what | runs where | spend |
|---|---|---|---|
| **T0** | unit and contract tests | `just check` (cargo test) | zero |
| **T1** | faux cassettes — behavior baseline, plan/goal/permission e2e over `faux/faux-1` | `just check` (D76 gate + suite) | zero |
| **T2** | real-binary journeys — the built `yi` driven end to end, offline | `just postmerge` + `postmerge-evals`, and the CI postmerge lane (§3) | zero |
| **T3** | paid smoke — one real-provider run per suite, ledgered in docs/eval-ledger.md with config fingerprint | opt-in, user-run, never in any gate | budgeted (plan law 3) |

The taxonomy changes no test; it names where each lives and closes the gap
the tiers expose: **every feature-ledger row names its journey test.** A row
that cannot name the T1/T2 test a user-visible regression would trip is
asserting "live" on vibes. Mechanically: a `journey:` clause in the row's
note column — new and edited rows carry it at land time; existing rows get
an annotation pass (S7). No mechanical gate over the prose table yet — a
grep-gate over a markdown table is brittle and the annotation pass has to
complete first; revisit once it has (open call, §10).

**The journey inventory** — what a user can actually do, and its test home:

| # | journey | drives | test today | tier | gap |
|---|---|---|---|---|---|
| 1 | ask one-shot | `yi ask --model faux/faux-1 "<prompt>"` (+`--json`) | evals/run.py `--dry` fixtures; behavior cassettes | T2/T1 | none |
| 2 | session resume + undo | `--continue`/`--session`, `yi undo` over T14 checkpoints | tui_drive rewind script; sessions/checkpoint tests | T1/T2 | resumed-session cost counter open (`A12`); no single scripted resume+undo binary journey — TODOS row §9 |
| 3 | kernel ipython + rlm child round-trip | `ipython` cell → `rlm.run` → child answer in parent namespace | recursion_e2e live-kernel round trip (phase-4 exit gate) | T2 | none |
| 4 | worktree isolate + merge | `rlm.run(isolation='worktree')` → `merge_worktree` | F5 runtime tests | T2 | none |
| 5 | ACP v2 client turn | scripted v2 client against `yi acp` | phase-5b scripted client (incl. `_yi/heartbeat_changed`) | T2 | terminal streaming open (`H1`) |
| 6 | TUI true-terminal | raw mode, CPR, kitty flags | scripts/tui_pty.py harness | T2 | run by hand only — wire into the postmerge lane (§9) |
| 7 | daemon multi-worker | `yi serve`, N workers × M kernels, reconnect | phase-6 exit gate (single); stress matrix unbuilt | T2 | `G2` — its named home (postmerge lane) now exists via §3 |
| 8 | plan/goal/check flow | plan DAG, `Task.check`, ladder, drain gate | plan_e2e, goal_e2e, behavior cassettes | T1 | none |
| 9 | permission ask flow | `yi gate <cmd>`, auto-mode fallback, M7/M8 review | gate battery; auto_review + review tests | T1/T2 | none |

## 3. Directive 3 — branch protection and PR standardization (D-row `Dβ`)

**Forge reality.** The remote is `github.com/jackmaxwil/yi`; PRs #9 and #11
are merged on main. docs/FORGEJO.md describes runners, secrets and a publish
path for a server that does not exist, and cites a deletion at the wrong
version. The doc must stop lying.

**What 0.60.0 actually decided** — and what survives it: the deletion refused
"a second unrun copy of the rules", not CI. What returns is a thin caller of
the same justfile the hooks run, which is FORGEJO.md's own doctrine ("a
workflow here is a thin caller — never a second copy of the rules"). The
reason survives; the forge changes.

**Restoration** (files under `.github/`):

- `workflows/pr.yml` — on `pull_request`: matrix `ubuntu-latest` +
  `macos-latest`, checkout with `fetch-depth: 0` (check_trailers.py needs
  `origin/main`), toolchain from rust-toolchain.toml, install `just`,
  `cargo-deny`, `cargo-machete`, `codespell`, python ≥ 3.11 — then one step:
  `just prepush`. GitHub sets `CI=true`, so check_binary_size (D70) and
  check_startup (D68) already skip themselves, announced not silent.
- `workflows/postmerge.yml` — on push to `main`, same matrix:
  `just postmerge` then `just postmerge-evals`. This is the T2 lane's away
  game and `G2`'s named home (`postmerge.yml` is what TODOS G2 has been
  pointing at since 0.60.0 — it now exists).
- `PULL_REQUEST_TEMPLATE.md` — the approved template, verbatim below.

**Branch protection** (GitHub settings on `main`; a human act, recorded in
the D-row): merges by PR only, force-push and direct push blocked, `pr.yml`
checks required, linear history required. This is `O6`'s enforcing half —
hooks fail in two seconds at the keyboard, protection catches `--no-verify`.

**Gate proof lives in CI status checks only.** A PR body never pastes gate
output; the checks tab is the proof, and a pasted green is exactly the
"guardrails: all green in a grep" the done-bar already refuses.

**The PR template, verbatim** (`.github/PULL_REQUEST_TEMPLATE.md`) — a
cold-reader narrative, zero checkboxes:

```markdown
<!-- Cold-reader narrative. Every section is prose a reviewer who was not in
     the session can follow. No checkboxes; gate proof is the CI checks tab,
     never pasted output. Delete a section only if it is truly empty
     (e.g. no UI change), and say so in Summary when it matters. -->

## Summary
<!-- What changed and why, in a few sentences a cold reader can follow
     without the diff open. -->

## User outcomes
<!-- What a user of yi can do, see, or rely on after this that they could
     not before. "Nothing user-visible" is a valid answer — say why the
     change exists anyway. -->

## UI changes
<!-- TUI/ACP-visible changes. For TUI: the headless frame dump or PTY
     evidence lives in the repo's test output; describe what moved. -->

## Files edited
<!-- A map, not a list: group by crate/area, one line each on why that
     area was touched. -->

## Schema changes
<!-- yi-types diffs, schemas.lock movement, fixtures added (never edited).
     "None" if none. -->

## LOC and justification
<!-- Net src LOC, measured. Over +150: this is the growth memo's home in
     PR form — what was weighed for deletion, why the bytes earn their
     place. -->

## Architecture notes
<!-- Version bump, changelog row, D-rows claimed or revised, feature-ledger
     rows touched (each names its journey test). -->

## Screenshots
<!-- Where a picture is the evidence (TUI frames, rendered output).
     "None" if none. -->
```

**docs/FORGEJO.md → docs/FORGE.md** (git mv + rewrite): the doctrine
survives verbatim where it can — the local-gate law ("Yi's gate is the
justfile, run from git hooks; the forge does not change that"), thin-caller
law, branch-protection list, the release-scoped-token rule, "what stays
local". The forge is named as GitHub; the workflow section describes the two
restored files; `check_startup.py` stays skipped on every runner (D68's
reason does not improve with a different forge). Forgejo-specific text
(runner labels, `just publish`'s Forgejo API, the secrets table's FORGEJO_*
rows) compresses to a short "if the forge moves" note — `just publish` and
`O2` keep their Forgejo shape in the justfile until a release actually needs
GitHub's API, which is its own later row, not this one. The 0.59.0 citation
dies with the rewrite (§0.1).

## 4. Directive 4 — the commit and PR-title standard

The standard (extends 090-workflow's "imperative subject" bullet; PR titles
follow it identically):

- One plain imperative sentence: **≤ 72 chars, first word capitalized, no
  terminal period**.
- **Self-evident to a cold reader**: it names the behavior or decision, not
  the campaign artifact — "Refuse the next done-claim on a rung-refused
  task", never "Close N14" or "Address feedback".
- **Ids never as titles.** D-rows, TODOS ids, U/P/J ids belong in the body.
- **Closed prefix vocabulary, exactly**: `Ratchet: ` (always with the
  measured `X -> Y`), plus git-native `Merge`/`Revert` subjects. Nothing
  else — **no conventional-commit dialect** (`feat:`, `fix:`, `chore:`,
  `docs:`, scoped or bang forms all fail).
- **No trailers**, with the two carve-outs §0.2 records: the plan-§8 `Opt-*`
  set on optimizer commits, and trailers git itself writes. Assistant
  co-author trailers stay banned.
- Body only when the why is not obvious from the diff (unchanged house law).

**Enforcement** rides `check_trailers.py` — it already owns the exact
plumbing (upstream resolution, `HEAD --not origin/main`, git's own trailer
parser) and the same scan span keeps history untouched. It grows subject
checks: length > 72; lowercase or non-letter first char (after an allowed
prefix); terminal `.`; conventional-dialect prefix `^[a-z][a-z-]*(\([^)]*\))?!?:`;
id-shaped subject (`^(D|[A-Z])\d+\b` with nothing else of substance). The
docstring widens from "no assistant co-author trailer" to the commit-message
law; the filename stays (renaming churns the aggregator for nothing — open
call §10). Red-first: commit `feat: add thing` and `Close N14` on a scratch
branch, quote both FAILs verbatim, drop them.

## 5. Directive 5 — the comment no-foreknowledge test

On top of the existing comment law (earned content only, ≤ 3 lines,
`Incident:`/`Invariant:` closed vocabulary, typed referents): **strip every
row and decision id from a comment and what remains must still fully carry
the fact.** Ids are optional trailing pointers; a pointer-only comment is a
defect — it outsources the fact to a document the reader at 3am does not
have open, and it rots the day the row is renumbered.

Before (defect — strip the ids and nothing remains):

```rust
// See D82 / TODOS N14.
```

After (the fact stands alone; the id is a courtesy tail):

```rust
// Incident: a rung refusal was laundered by resume — the streak was born
// empty on every --continue, so the red count rides the plan fact (D82).
```

**Enforcement** rides `check_comments.py` (it already walks every run):
strip id tokens (`\b[A-Z]{1,2}\d+\b`, `§[\d.]+`, `TODOS`, `docs/plans/\S+`)
and pointer stop-words (`see|per|cf|ref|row|the|of|in|and|for|to|a`) from a
run's text; a run left with fewer than three substantive words is flagged
`pointer-only comment`. Hard fail, zero-start — no baseline, so the sweep of
existing pointer-only comments lands **in the same commit as the detector**
(a gate cannot be committed red; O9's migrations-with-gate pattern). The
license-header exemption is untouched. Red-first: plant `// see D82.` in a
src file, quote the FAIL, remove.

**Judgment — no separate D-row.** The extension narrows what an earned
comment may contain; it adds no mechanism class, no schema, no dependency.
It rides the governance landing's changelog row, citing the comment-law
lineage (D49 enforcement, D55 typed referents). If the reconcile agent
judges the detector structural, it claims the next number — nothing below
depends on this staying row-less.

## 6. D-row texts (final; numbers are placeholders — claim at integrate)

Drafted as `Dα`/`Dβ`/`Dγ`; at 0.79.0 the next free numbers are D83/D84/D85,
which a parallel agent may consume first. Claim against the live tail, in
this order.

**`Dα` — net src LOC is budgeted per version** · *decision:* ≤ +150 net src
LOC per version rides free; beyond it the version's changelog row carries a
`growth:` memo naming the measured number and the deletion weighed; beyond
+2000 the landing additionally claims a D-row. `check_growth.py` enforces it
over `baselines/src_loc.json` (`{version, loc}`, seeded 0.79.0/52,150),
measuring the same `src_files()` set every size ratchet reads; `--update` in
its own commit per the standing baseline law. · *why:* 17 measured bumps
split into two regimes — organic median +35, landings +643..+3295 — so the
per-file and per-crate ratchets bounded the pieces while the repo's total
slope had no price; a flat cap would strangle landings or gate nothing, and
a memo prices growth instead of forbidding it. · *reversible via:* delete
the gate and baseline; written memos survive as ordinary changelog prose.

**`Dβ` — the forge is GitHub, and `.github/` returns as thin callers** ·
*decision:* restore `.github/`: `pr.yml` (`just prepush`, Linux + macOS
matrix, on pull request), `postmerge.yml` (`just postmerge` +
`postmerge-evals` on push to main), `PULL_REQUEST_TEMPLATE.md` (cold-reader
narrative, zero checkboxes; gate proof lives in CI status checks only).
Branch protection on `main` — PR-only, `pr.yml` required, linear history —
is `O6`'s enforcing half. `docs/FORGEJO.md` becomes `docs/FORGE.md`: the
local-gate and thin-caller doctrine survives, the forge is named as GitHub,
Forgejo specifics compress to a move-note. · *why:* the forge reality is
GitHub with live merged PRs, and the doc described runners that do not
exist while citing the deletion at the wrong version (0.59.0; the row is
0.60.0). 0.60.0 deleted a *second unrun copy of the rules*; what returns is
a caller of the same justfile the hooks run, so the deletion's reason is
honored, not reversed. D68/D70's CI skips already fire on `CI=true`. ·
*reversible via:* delete `.github/` and revert the doc move; protection is a
settings change no code depends on.

**`Dγ` — tests are tiered, and a ledger row names its journey** ·
*decision:* T0 unit / T1 faux cassettes (`just check`) / T2 real-binary
journeys (`just postmerge` + the CI postmerge lane) / T3 paid smoke
(opt-in, user-run, ledgered in docs/eval-ledger.md; never in any gate —
plan law 3 unchanged). Every feature-ledger row names its journey test in a
`journey:` clause; new and edited rows at land time, existing rows in one
annotation pass. The nine-journey inventory (governance plan §2) is the
coverage map. · *why:* the ledger asserts "live" with no named proof, and
the taxonomy makes the zero-spend line structural: what `just check` may
run, what needs a binary, and what costs money are three different answers
that were previously folklore. · *reversible via:* drop the taxonomy text
and `journey:` clauses; every test remains an ordinary test.

## 7. The exact .ruler edits

Instruction source of truth is `.ruler/`; the generated files are never
edited (100-never law). Edits, by file:

- **`040-guardrails.md`** — append two bullets: the growth budget (free band
  +150, `growth:` memo beyond, D-row beyond +2000, `check_growth.py` +
  `src_loc.json`, `--update` own-commit law applies) and the
  **deletion-first clause**: "Growth is paid for before it is excused: a
  landing past the free band names, in its memo, what was weighed for
  deletion — the ratchet is a ceiling, not a target, and the budget is a
  price, not a permission."
- **`080-testing.md`** — append the tier taxonomy (the §2 table as four
  terse lines) and the **journey clause**: "Every feature-ledger row names
  its journey test; a new or edited row without a `journey:` clause is not
  done. T3 spends money and therefore never sits in any gate."
- **`030-style.md`** — extend the comment-law bullet with the
  **no-foreknowledge test**: "Strip every row and decision id from a
  comment and what remains must still fully carry the fact; ids are
  optional trailing pointers, and a pointer-only comment is a defect
  check_comments.py rejects."
- **`090-workflow.md`** — replace the single "Commits: imperative subject"
  bullet with the §4 standard (≤72, capitalized, no period, cold-reader
  self-evident, ids never as titles, closed prefix vocabulary `Ratchet:` +
  `Merge`/`Revert`, no conventional-commit dialect, no trailers with the
  two recorded carve-outs; PR titles identical). Add one bullet: "PR bodies
  are the cold-reader narrative template — zero checkboxes; gate proof
  lives in CI status checks only."

Then, in each worktree that needs the regenerated instructions:
`npx @intellectronica/ruler apply`. What moves: the untracked generated
files for the three `default_agents` (`claude` → CLAUDE.md, `the reference` →
AGENTS.md, `pi` → its agent file) plus the propagated skill directories —
all untracked/gitignored, so `git status` stays clean and nothing is staged.

## 8. File-ownership matrix and sequencing (builders S3–S8)

No two builders touch the same file. ARCHITECTURE.md, TODOS.md and version
numbers are touched **only at integrate** (S8), which claims D numbers and
the version against a freshly read header.

| builder | owns | delivers |
|---|---|---|
| **S3** | scripts/guardrails/check_growth.py (new) · baselines/src_loc.json (new, own commit) · check_guardrails.sh (one `run` line) | the growth gate, red-first-proven (§1), seeded 0.79.0/52,150 |
| **S4** | scripts/guardrails/check_trailers.py | subject-standard checks (§4), red-first-proven on a scratch branch |
| **S5** | .github/workflows/pr.yml · .github/workflows/postmerge.yml · .github/PULL_REQUEST_TEMPLATE.md · docs/FORGEJO.md → docs/FORGE.md | the forge restoration (§3); the protection settings are listed for the user, not scripted |
| **S6** | scripts/guardrails/check_comments.py · every src file the sweep touches | pointer-only detector + sweep, one commit, red-first-proven (§5) |
| **S7** | .ruler/{030-style,040-guardrails,080-testing,090-workflow}.md | the §7 edits, then `ruler apply` |
| **S8** (integrate) | docs/ARCHITECTURE.md (version, changelog row(s), D-rows, feature-ledger `journey:` annotation pass) · docs/TODOS.md (§9 rows) · COMMIT_PLAN.md if commits are denied | claims real D numbers, reconciles any collision, lands the docs |

Sequencing: **S3, S4, S5, S6 in parallel** (disjoint files). **S7 after S6**
— the comment-law wording must match what the detector actually enforces.
**S8 last**, always: it reads the live header at the moment of writing, and
if the reconcile agent moved the tail, it renumbers here and nowhere else.
Ordering law inside every builder: gate code lands red on baselines, then
`--update` alone (S3's seed commit); a gate never lands red on the tree
(S6's sweep rides the detector's commit).

Commit plan shape (subjects per §4; final paths per builder above):

1. `Budget net src growth per version` (S3 gate + aggregator line)
2. `Ratchet: seed src LOC baseline at 52150` (S3, baseline alone)
3. `Enforce the commit subject standard on new commits` (S4)
4. `Restore GitHub workflows as thin callers of the justfile` (S5)
5. `Rename the forge doc to match the forge` (S5, the git mv + rewrite)
6. `Reject pointer-only comments` (S6, detector + sweep)
7. `Carry the governance laws in the instruction source` (S7)
8. `Record the governance package` (S8: version, changelog, D-rows, TODOS,
   ledger annotations)

## 9. TODOS updates (written by S8)

- **`O1`** — partially closes: "done" gains its first half — `pr.yml` runs
  `just prepush` on Linux **and** macOS (GitHub-hosted matrix), so a
  Linux-only break is now caught. Still open: musl cross target installed
  and exercised, `package-all` refusing to report a skipped target.
- **`O6`** — the open line ("hooks are bypassable with `--no-verify`; the
  enforcing half is Forgejo branch protection") closes when the user flips
  GitHub protection on `main` per `Dβ`; the row records the settings list
  and points at docs/FORGE.md.
- **`G2`** — reword: the home (`postmerge.yml`) now exists; what remains is
  the stress matrix itself.
- **`K1`** — correct "the forge is moving to Forgejo" to the GitHub reality;
  the bridge target is `gh` (which the feature-ledger row already said).
- **`J1`** — note: the postmerge lane the ledger row waits on now runs in CI
  on every push to main; the human paste and the paid run stay human.
- **New `O11` — journey gaps** · S · from §2's inventory: (a) a scripted
  resume+undo real-binary journey (journey 2), (b) scripts/tui_pty.py wired
  into the postmerge lane (journey 6). done: both run in `just postmerge`
  or a named sibling, and the inventory table carries no "gap" cell for
  either.
- **New `O12` — T3 paid smoke tier** · S · opt-in, user-run, one
  real-provider run per suite, one ledger row each (docs/eval-ledger.md),
  zero API in any gate (plan law 3). done: the first T3 row exists and
  names its config fingerprint; overlaps `J2`/`J3`'s user-run halves and
  says so rather than duplicating them.
- **New `O13` — feature-ledger `journey:` annotation pass** · S · every
  existing ledger row gains its clause (S8 starts it; any row whose journey
  test does not exist points at `O11` or opens its own row). done: no row
  without a clause; then the mechanical-gate question (§10) is decidable.

## 10. Not building, and open calls

- **No mechanical gate over the PR body or the ledger's `journey:` clauses
  yet** — prose-table greps are brittle; decide after `O13` completes.
- **No rename of check_trailers.py** — the name understates it now
  (commit-message law, not just trailers); renaming churns the aggregator
  for zero behavior. Open call for a later janitorial commit.
- **No GitHub release/publish port** — `just publish` keeps its Forgejo
  shape until a release actually needs GitHub's API; that is its own row,
  not this campaign.
- **No new deps, crates, env vars, CLI verbs** — every gate here is stdlib
  python over the existing aggregator; CI installs only what the gate
  already requires locally.
- **No LLM anywhere in these gates** (plan law 2): growth, subjects,
  comments and tiers are counters, regexes and tables.
