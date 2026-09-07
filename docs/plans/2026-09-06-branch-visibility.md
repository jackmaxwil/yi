# Where the work is: worktree and branch visibility in yi

Status: built as D143 and D144 (0.175.0), in the order §5 names: the branch
reader, then the lane line, then the two verbs, with §6's amendments. Departures
from §4, and why: `yi doctor` still fails on every left slot and still names
`yi lanes reap N`, because its `--fix` is the script-side answer and a row that
passes on a slot the next claim will take would hide the slot from `--fix`; the
prompt writes no D81 row, because that ledger is the permission broker's and the
broker does not exist before the claim (the prompt's own line on stderr is the
receipt); the ledger join runs in `yi lanes` only, because the runtime cannot
reach the daemon's file (§6.1 finding 11); the `land` tool for the model is not
built (§4.2, phase two).

Provenance: a design investigation into what a person and a model can see about
the lane, the branch, the base and the pull request a session works on, after
D119–D124 (0.148.0) made every root session worktree-first. The evidence is the
code at this commit and the live pool on the machine that wrote this, which
today reads:

```
lane 0: left by pid-69176 on yi/pid-69176 — `yi lanes reap 0`
lane 1: left by pid-63020 on yi/pid-63020 — `yi lanes reap 1`
lane 2: idle at 914a5ba70009
```

Slot 0 holds one untracked path (`.yi/plans/`), zero commits ahead of
`origin/main`, 55 behind, and its branch was created 20 hours ago. Slot 1 is
clean, zero ahead, 45 behind, 18 hours old. Neither process is alive. The
listing says none of that.

## The one-line finding

**Every surface that says where the work is was built for the session that
claimed the lane, and none of them was built for the person who comes back
later, or for the sibling that never claimed one.** The status row and the
environment block are right for the live session. `yi lanes` is the only view
of the pool, and it prints a slot number, a session id and a branch name that is
the same session id again; it never prints the path on disk, the state of the
tree, the age, or who the holder is in terms a person recognises.

## 1. Analyze

### 1.1 Surfaces, by reader

Invariant: the model's surface and the person's surface are different sets. The
model sees the tree state and the lane line; the person sees the branch and the
landing. Neither sees both.

For the person:

| surface | shows | source of each field | file:line |
|---|---|---|---|
| status row, path segment | `<cwd>@<branch>`, both shrunk from the left under one 24-column budget | `cwd` is the lane path (`shells.rs:64-70`); `branch` is `git_branch(cwd)`, a hand parse of `.git/HEAD` | `crates/tui/src/status.rs:96-108`, `:161-190` |
| status row, landing segment | `pushed`, `PR #N ●●⟳ · main +4`, `PR #N merged`; nothing while unlanded | `app.landing`, set only by `AgentEvent::LandingState` | `status.rs:144-159`; `crates/tui/src/app.rs:181`, `:607` |
| HUD row | `Land · <landing_line>` while a landing exists | same `app.landing` through `slash::landing_line` | `crates/tui/src/hud.rs:113-117` |
| `/lanes` | one line per slot: `idle[, warm] at <sha12>` / `held by <session> on <branch>` / `left by <session> on <branch> — yi lanes reap N` | `Pool::list` → `format_lanes` | `crates/runtime/src/slash.rs:157-172`; `crates/runtime/src/lane/land.rs:396-398`, `:438-470` |
| `/pr` | the landing line after a forge poll | `LaneHandle::refresh` | `slash.rs:164`, `:174-193`; `land.rs:363-386` |
| `/land`, `/base`, `/discard` | one confirmation line each | | `slash.rs:161-167` |
| `yi lanes`, `yi lanes reap N` | the same `format_lanes` text; the reap note (`branch kept (unmerged)` / `deleted (merged)`) | `Pool::list`, `Pool::reap` | `crates/cli/src/lanes.rs:53-87`; `lane/mod.rs:545-562` |
| `yi doctor`, row `lanes` | `FAIL lane N left by S; yi lanes reap N`, or `fixed`, or `N slot(s) consistent` | `Pool::list` with `DEFAULT_SLOTS`, not the configured count | `crates/cli/src/doctor.rs:143-183` |
| quit line | `lane N · branch B off <base12> · land with /land "Title"` then `Resume this session with yi --session <id>` | `LaneHandle::describe` | `crates/cli/src/shells.rs:127-135`; `land.rs:243-252`; `main.rs:847-852` |
| claim refusal | `no free lane: H held, O orphaned of N; yi lanes lists them` | `LaneError::PoolFull` | `lane/mod.rs:92-97`; `main.rs:450-458` |
| steers | `gate red on pull request #N: job …`, `landing failed: …`, `gate poll failed: …` | the land thread | `land.rs:286-290`, `:352-357`, `:372-381` |
| console | `_yi/landing` is forwarded; the sidebar shows name, state glyph and recency per session, no lane, no branch | | `crates/acp/src/update.rs:378`; `crates/console/src/sidebar.rs` |

For the model:

| surface | shows | file:line |
|---|---|---|
| environment block, line 1 | `cwd: <lane path> (git: <branch>, N modified)` from `git status --porcelain --branch` under a 5 s probe | `crates/runtime/src/environment.rs:14-29`, `:113-116` |
| environment block, line 2 | `lane N · branch yi/<id> off <base12> · land with /land "Title"` | `environment.rs:117-119`; `land.rs:243-252` |
| steers | the same three steers the person sees | `land.rs` as above |

What each reader has that the other lacks:

- The model sees the modified-file count. The person never does.
- The person sees the landing (pull request number, job glyphs, `main +N`). The model never does; the environment block has no landing line, and `/land` is a person's verb, not a tool (`slash.rs:1`, and no tool in `crates/tools` names it). The lane line tells the model to land with a verb it cannot run.
- Neither sees ahead-of-`main` at any time, or behind-`main` before a pull request is open. `behind` is computed only inside `poll_forge` (`land.rs:186-190`) and only from the last fetched `origin/main`.
- Neither sees the trunk checkout's path once a lane is claimed. The session file is keyed by the trunk cwd (D119) but every path on screen is the slot.

### 1.2 The lifecycle and where each fact lives

```
claim ─── work ─── land ─── merge ─── release | reap
```

| step | git worktree list | `.held` flock | `<n>.json` (`SlotState`) | daemon ledger | memory |
|---|---|---|---|---|---|
| claim (`mod.rs:402-483`) | `worktree add --detach` or `reset --hard` + `clean -fd`; `checkout -B yi/<sid>`; `worktree lock --reason session:<sid>` (`let _ =`) | created, flocked by this process (`:310-321`) | `base = sha`, `session = sid` | untouched | `Lane { slot, path, branch, session }` |
| bind (`mod.rs:638-656`, `main.rs:836`) | branch renamed to `yi/<store id>`; unlock + lock again (`let _ =`) | held | `session = store id` | the worker's `session/new` result names the session | `Lane.branch`, `Lane.session` |
| work | HEAD moves with commits; the tree dirties | held | unchanged | `lastState`, `lastEventMs`, `unseen`, `name` | `app.branch` re-read on every assistant `MessageEnd` (`app.rs:395`) |
| `/land` (`land.rs:277-361`) | branch pushed | held | unchanged | unchanged | `Landing::Pushed` → `Open { pr, jobs: [], behind: 0 }` → polled every 60 s for 2 h |
| `/land` again on all-green | | | | | `Landing::Merged` |
| release (`mod.rs:691-706`) | `worktree unlock`; `checkout --detach`; branch deleted iff `merge-base --is-ancestor branch main`; warmer starts | dropped with the `File` | `session = None` | row stays with `lastState: idle` | gone |
| crash | lock reason stays `session:<sid>` | dropped by the kernel | `session = sid` | row stays; reloads as `idle` (D118) | gone |
| reap (`mod.rs:545-562`) | as release, without the warm | free | `session = None` | untouched | — |
| resume (`mod.rs:485-499`) | `checkout yi/<sid>` | flocked again | `session = sid` matched | row reused | `Landing::Unlanded` (`land.rs:222`) |

Places where two sources can disagree, with the evidence that at least one of
them does today:

1. **`Pool::list` versus `Pool::claim` on an orphan.** D130 put the
   `abandoned` predicate (clean tree, branch an ancestor of `main`) in `claim`
   only (`mod.rs:423-430`). `list` still reports such a slot as `Orphan`
   (`:348-352`), `format_lanes` prints the reap hint, and `yi doctor` prints
   `FAIL`. Slot 1 today is exactly this: clean, merged, listed as left, and the
   next claim will take it without a word.
2. **The `.held` mtime versus the claim time.** All three `.held` files today
   carry one mtime to the second (18:43:38), while the two branches were
   created at 22:53 yesterday and 00:29 today by their reflogs. The file is
   opened with `File::create` on every probe (`:312`) and says nothing about
   when the slot was taken. There is no claim time anywhere except git's
   reflog for the branch, which has it exactly.
3. **The `worktree lock` reason versus `<n>.json`.** D120 calls the reason the
   crash record. Nothing reads it: no crate runs `git worktree list`. It is
   written with `let _ =` at claim and at bind, so a failed lock leaves the
   previous session's name under the new session's slot with no reader to
   notice.
4. **The branch name versus the session id on the daemon path.** `bind_session`
   is called from `attach_store` in the CLI (`main.rs:836`). The ACP worker
   attaches its store at `crates/acp/src/lib.rs:305` and never binds. Every
   console session therefore stays on `yi/pid-<worker pid>` with
   `session = pid-<worker pid>` in the state file. `yi --session <id>` can never
   reclaim it (`claim` compares the state file's session to the store id,
   `mod.rs:418`), and `format_lanes` names it by a pid. The two orphans today
   are this case. Predicted from the code and not reproduced: a second
   `session/new` on the same worker claims a second slot under the same branch
   name, and `checkout -B` refuses a branch checked out in another worktree,
   so the second session's start fails with a git error.
5. **Three readers of "what branch am I on".** `status::git_branch` parses
   `.git/HEAD` by hand and returns an 8-character sha when detached
   (`status.rs:186-189`). `environment::git_summary` spawns `git status
   --porcelain --branch` and returns the word `detached` (`environment.rs:19-27`).
   `Pool::branch_of` spawns `git symbolic-ref --short -q HEAD` under the
   120-second lane timeout and returns `None` when detached (`mod.rs:323-331`).
   Three parses, three spellings of one state, three timeouts (none, 5 s,
   120 s).
6. **`Landing` in memory versus the forge.** The poll stops after two hours
   (`land.rs:18`, `:346`) or on the first poll error (`:351-357`). After that the
   status row keeps showing the last glyphs with no age. On resume the handle
   starts at `Unlanded` although the pull request is open on the forge.
7. **`behind` versus `main`.** `poll_forge` counts `HEAD..origin/main`
   (`land.rs:186`) without fetching. `origin/main` moves only at claim
   (`fetch_if_stale`, `:366-377`) and at `/base` (`:391`). `main +0` after a
   day means nothing was fetched, not that nothing landed.
8. **`yi doctor` versus `lanes.slots`.** `doctor.rs:145` opens the pool with
   `DEFAULT_SLOTS`; `lanes.rs:57-59` opens it with the configured count. A repo
   configured for five slots has two the doctor cannot see.
9. **The transcript's `branch` versus git's.** The compact context view renders
   a rewind summary as `branch: <summary>` (`crates/context/src/view.rs:212`), in
   the same context window as `(git: <branch>, N modified)`. The collision is
   in the model's input, not on the person's screen; the tree view and the
   session tree say "branch" nowhere a person reads.

### 1.3 Not visible anywhere

Each item names where the fact exists and why no surface prints it.

- **The slot's path.** `format_lanes` prints the slot number only. The
  status row prints the path shrunk to its last 24 characters:
  `…/897d6e9162485667/1@yi/01a06f8d…`, a hash and a digit. The plan doc
  promised that the quit line, the session list and `/lanes` would all say
  path, branch and pull request; none of the three prints the path.
- **Which repository.** The pool directory is a content hash of the canonical
  repo root (`mod.rs:245-247`). No surface maps it back to a name. Two TUIs on
  two repos both read `…/<hash>/0`.
- **The tree state of a slot.** Modified and untracked count, ahead and behind.
  Git has all three; `list` asks for none. Slot 0's untracked path is the only
  copy of it.
- **The age of a lane.** Not recorded. Derivable from `git reflog
  refs/heads/yi/<id>` (branch creation) and `for-each-ref --format=%(committerdate)`.
- **Who a holder is.** `held by 01a06f8d-…` is the store id. The ledger has the
  first-prompt `name` and the root `cwd` for that id (`DaemonLedgerEntry`,
  `crates/types/src/acp.rs:137-147`); nothing joins them.
- **Whether an orphan's branch has a pull request.** The forge knows it by head
  branch; nothing asks.
- **When the landing was last refreshed, and whether the poll is still running.**
- **Cross-session state.** This repo runs 42 worktrees on one `.git` today (the
  `.claude/worktrees` layout plus the pool plus stale drive slots under `/T/`).
  Which of them have a live session is in the ledger; which lane each holds is
  in the pool; nothing shows either list beside the other.
- **The pool from inside a `--here` session.** `/lanes` refuses (`slash.rs:159`)
  although `Pool::open(cwd)` works from the trunk, which is what `yi lanes` does.

## 2. Critique

Each item carries the moment it costs a real person something. An item with no
such moment was cut.

### 2.1 `yi lanes` cannot tell a safe reap from a lossy one

`format_lanes` prints `left by pid-69176 on yi/pid-69176 — yi lanes reap 0`.
Reap runs `checkout --detach`, which carries a modified file into the detached
tree, then the next claim runs `reset --hard` and `clean -fd` (`mod.rs:459-460`).
D121's invariant, "an orphan's branch is the only copy of its work", covers
commits and not the working tree. Slot 0 today has one untracked path and zero
commits ahead; its reap note will say `branch deleted (merged)` and the path is
gone at the next claim. D130 weighed exactly this case ("a dirty tree whose
changes are only untracked files") and left it to the person, but gave the
person no line to read. Cost: the last copy of an uncommitted file, lost at a
moment the tool said was routine.

### 2.2 Three branch readers, three answers, three timeouts

Listed at 1.2 item 5. The moment: a detached slot reads `914a5ba7` on the status
row, `detached` in the model's environment block, `(detached)` in `/lanes`. A
person comparing what they see with what the model says it sees cannot match
them. The heavier cost is the timeout: `/lanes` runs on the port call
(`crates/tui/src/port.rs:213-220`), and `Pool::list` spawns `git symbolic-ref`
per slot under a 120-second deadline. A git wedged on a sibling's index lock
holds the TUI for up to six minutes on a three-slot pool. `git_branch` reads a
file and needs no spawn; it is the reader that should survive, moved to the
runtime so the environment block and the pool use it too. `git status` stays for
the modified count only.

### 2.3 `path@branch` repeats the session id and drops the repo

The branch is `yi/<store id>`. The right edge of the same row shows the store id
again, in the tile. The 24 columns of `…/897d6e9162485667/1@yi/01a06f8d…` carry
one new fact, the slot digit. The trunk path, the one directory the person can
type into an editor, is absent. The moment: two terminals, two repos, both rows
read `…/<hash>/0`; the person opens the wrong one. Cost: minutes and a wrong
edit in the wrong tree. For a lane session the row should read `yi ⎇ lane 1`
in the columns the hash uses now; `--here` keeps `path@branch`.

### 2.4 A stale landing reads as a live one

`PR #191 ●●●` stays on the row after the two-hour poll ends, after a poll error,
and across a resume it never appears at all. The moment: the person comes back
the next morning to a green row, types `/land "Title"` expecting the merge, and
`land_blocking` sees `Unlanded` (resume) so it pushes and asks the forge to open
a second pull request for a head that already has one. The forge refuses; the
person gets `landing failed: …`. The forge held the truth the whole time and is
one `pr list --head` away. Cost: a confusing refusal at the exact step the
feature exists to make trivial.

### 2.5 Orphans accumulate silently until the pool is full

A hard kill frees the flock and the person's terminal is gone; nothing is
printed. The next `yi` claims another slot. Only the N+1th start says
`no free lane: 0 held, 3 orphaned of 3; yi lanes lists them`, which is the one
place the reap verb is discoverable. That refusal is good. The gap is that the
two prior orphans were invisible and one of them is dirty. `yi doctor` finds them
but nobody runs `doctor` before a session start. Cost: a refusal on the third
start, and by then two slots' worth of decisions to make at once.

### 2.6 The daemon path names lanes by pid and cannot reclaim them

Item 4 at 1.2. The moment: the console is restarted (`just dev` rebuilds), the
worker dies, two slots are left under `pid-` names, and the person's `yi
--session <id>` from the terminal claims a fresh third slot instead of the one
holding that session's commits, because the state file names a pid. The listing
then shows three lanes for one piece of work and the person cannot tell which
is which. Cost: the branch with the work sits under a name nothing will look up
again, until reap keeps it as `yi/pid-63020` forever.

### 2.7 "branch" is overloaded in the model's input, not the person's

Item 9 at 1.2. The person never sees the word for a transcript branch. The model
sees `branch: <summary of an abandoned attempt>` in the compact view beside
`(git: yi/01a0…, 3 modified)`. Cost: small and real; a model asked "what branch
are you on" has two candidate answers in context. The fix is a word in
`view.rs:212` (`earlier attempt:`), no design.

### 2.8 Cross-session state is an absence, not a scope decision

The plan doc's ergonomics section wanted `/lanes`, the quit line and the session
list to agree. The ledger (D118) was built for the console's sidebar and holds
root, name, state and recency per session id. The pool holds session id per
slot. The join is one map lookup once the ACP path binds the store id (2.6).
The moment: 11 worktrees, three of them running agents, and the person asking
"which of these is the one doing the auth work" gets a pid. Cost: the person
opens sessions one by one to find out.

### 2.9 The `--here` refusal is right for the writers and wrong for the readers

`/land`, `/base`, `/discard` write to a lane; refusing without one is correct.
`/lanes` reads the pool, which is keyed by the common git dir, and the trunk
is in that repo. `/pr` could report the trunk branch's pull request from the
forge. The moment: a person in `--here` trying to see whether the pool is full
before starting a second session, told "no lane". Cost: one detour to a second
terminal; low, and the fix is one arm in `lane_verb`.

## 3. Options

### 3.1 Generated wide

| option | verdict |
|---|---|
| A. Docs only: write the pool path and the reap rule into `docs/` | killed as a fix; the facts are needed on screen at the moment of the decision, and the pool path is a hash nobody can type from memory. Kept as a rider: the lane line and the reap note name the path. |
| B. Enrich `format_lanes`: path, modified count, ahead/behind, age, holder's ledger name and root, clean-and-merged marker | survives; see 3.2 |
| C. A dedicated `yi lanes --long` or a second view | killed; two renderers of one list drift, and B is the view |
| D. A TUI lanes overlay | killed; the row is under a cascade already (`status.rs:210-240`), `/lanes` already renders a cell, and an overlay is a third copy of B |
| E. Lane form on the status row: `repo ⎇ lane N` replaces `path@branch` for lane sessions | survives as a rider on B: same facts, one fewer duplicate of the session id |
| F. A landing line in the environment block for the model | survives, narrow: one line, only when `Open` or `Merged`, from memory. Without it the model recommends opening a pull request that exists. More than one line is a regression: the environment block is hash-pinned and every line is paid every turn |
| G. Cross-session `yi lanes` through the ledger join | survives, inside B; needs 2.6 fixed first |
| H. `yi lanes doctor`: name disagreements between git, `.held`, `<n>.json`, ledger | killed as a new verb; folded into the existing `doctor` row `lanes`: read `lanes.slots`, flag a state-file session that is not the branch's suffix, flag a `pid-` holder, and stop failing on a clean-and-merged orphan |
| I. Landing reattach from the forge on resume, plus an age on the segment | survives; see 3.2 |
| J. One branch reader | survives; see 3.2 |
| K. Fetch `origin/main` inside the poll so `behind` is true | killed; the poll would then reach the network twice a minute, and a wrong `+0` is cheaper than a fetch storm across 11 workers on one `.git`. The age marker from I says how old the number is |
| L. Record the claim time in `<n>.json` | killed; new persisted field, and git's reflog already holds it |
| M. Reap refuses a dirty tree | killed; a reap that refuses is a pool that stays full, and the person who typed reap after reading a listing that says "1 modified file" has decided. The listing is the control |
| N. `--here` degrades `/lanes` and `/pr` to reads; the writers keep refusing | survives as a rider on B |
| O. Rename the transcript-branch prefix in the compact view | survives as a one-word change; no ADR |

### 3.2 Pressure test of the survivors

**J. One branch reader.** Move `status::git_branch` into `runtime::environment`
as the reader of HEAD; `git_summary` uses it for the branch and keeps `git
status --porcelain` for the count; `Pool::branch_of` uses it for the slot.

- Breaks: the detached spelling becomes one string in three places; no test
  pins the current spellings (`tui_unit` status cases pass `StatusInput`
  directly; `lanes.rs:269` asserts on an idle line).
- Maintains: −30 LOC in `status.rs`, −10 in `mod.rs`, +5 in `environment.rs`.
- New state: none.
- Slow git: `list` loses three spawns under a 120 s deadline; the HEAD read is a
  file read. `git status` stays behind the 5 s probe, per prompt, never on
  render.
- Duplicates: removes two.
- ADR: none; it is a deletion. One changelog row.

**B (+E, +G, +N). One lane line that says where the work is.** `SlotView` gains
`path`, `modified: u32`, `ahead: u32`, `behind: u32`, `age` (from the branch's
reflog), and for `Held` and `Orphan` the ledger's `name` and `root` when the
session id is in the ledger. `format_lanes` prints one line per slot:

```
lane 0  ~/.yi/lanes/897d…/0  left by pid-69176  yi/pid-69176  1 untracked · 0 ahead · 55 behind  20 h  reap keeps the branch; the untracked path is lost
lane 1  ~/.yi/lanes/897d…/1  left by pid-63020  yi/pid-63020  clean · merged  18 h  the next claim takes it
lane 2  ~/.yi/lanes/897d…/2  idle, warm at 914a5ba70009
```

The status row for a lane session reads `yi ⎇ lane 1`; the quit line prints the
same line as `/lanes` for its own slot; `/lanes` under `--here` lists the pool.
The ACP worker calls `bind_session` after `attach_store` (`lib.rs:305`), so a
console session's lane carries its store id and the ledger join works.

- Breaks: `journeys.rs:368-373` (`starts_with("lane 0: idle")` and the 12-char
  sha) and `lanes.rs:269` need the new prefix; `tui_unit` gains one status case
  for the lane form; `cli_surfaces` `a_headless_drive_claims_no_lane_unless_asked`
  is untouched.
- Maintains: about +80 LOC in `lane/mod.rs` and `land.rs`, +15 in `slash.rs`,
  +10 in `status.rs`, +5 in `acp/lib.rs`; the growth memo carries it.
- New state: none. Git holds the tree facts and the age; the ledger holds the
  name and root; `<n>.json` is read, not extended.
- Slow git: every per-slot read goes through `capture` with the 5 s probe and
  degrades its field to `?`; the forge is never called from `list`, so a forge
  outage cannot touch it.
- Duplicates: replaces the current line, the quit line's `describe`, and the
  doctor's reap hint with one renderer.
- ADR: **D143**, "a lane line says path, holder, tree state and age from git
  and the ledger, never from a new file".

**I. Landing reattaches from the forge and shows its age.** On `LaneHandle::new`
with a lane, and on `/pr` while `Unlanded`, ask the forge for an open pull
request whose head is the lane's branch (`tea pr ls --state open` filtered by
head, or `gh pr list --head`). Found: `Open` with jobs from the existing poll.
`LaneHandle` keeps the `Instant` of the last successful refresh;
`landing_segment` appends `· 3 h ago` when it is older than two poll periods.

- Breaks: nothing pinned; `lanes.rs:318` (`forge_replies_parse_to_typed_numbers_and_slugs`)
  gains a fixture for the list reply.
- Maintains: about +40 LOC in `land.rs`, +5 in `status.rs`.
- New state: an `Instant` in memory. No file.
- Forge down: the reattach fails quietly and the handle stays `Unlanded`; the
  segment stays absent. The row never claims a pull request it did not see.
- Duplicates: none; it feeds the segment and the HUD row that exist.
- ADR: amend D123 in place, one row: "the landing is recovered from the forge
  by head branch on resume, and its segment carries an age".

**F. One landing line for the model.** `environment.rs` appends
`landing: PR #191 open · lint ● test ⟳ · main +4` from the in-memory `Landing`
when it is `Open` or `Merged`. Zero spawns, zero state. Rides with I; no ADR of
its own.

**O. `earlier attempt:` in the compact view.** One word at `view.rs:212`. Rides
with whichever lands first.

Killed after the test: C, D, H-as-a-verb, K, L, M. Each survived only by
sounding tidy.

## 4. The simpler flow

The verbs above fix what each surface shows. They leave the vocabulary alone,
and the vocabulary is the larger cost. A new person today meets nine things
before the first landing:

| today | reader | what it is |
|---|---|---|
| `/lanes` | person | list the pool |
| `/land "Title"` | person | push, open, and on green, merge |
| `/pr` | person | refresh the landing |
| `/base` | person | merge `origin/main` in |
| `/discard` | person | delete the branch, keep the transcript |
| `yi lanes` | person | the same list from a shell |
| `yi lanes reap N` | person | free an orphan |
| `yi doctor --fix` | person | free every orphan |
| `--here` | person | no lane |
| `lane N · branch … · land with /land "Title"` | model | a verb it cannot run |

Invariant for the redesign: one word per intent, the same word for the person
and the model, and no word for a state the tool can resolve itself.

### 4.1 What the person learns

Two verbs. Everything else is a state the row shows or a question yi asks.

| intent | verb | absorbs |
|---|---|---|
| where am I | nothing: the status row reads `yi ⎇ lane 1`, or `PR #191 ●●⟳ · 3 h ago` once landing | `/pr` |
| ship it | `/land "Title"` | `/base` (fetch and merge `origin/main` before the push; a conflict stops with the file list and no push), `/pr` (`/land` with no title prints the landing line instead of `needs a title`), and the green-merge it already does |
| throw it away | `/discard` | unchanged; a destructive verb keeps its own word |
| what is in the pool | `/lanes` in a session, `yi lanes` in a shell; one renderer (D143) | `yi doctor`'s reap hint |
| a stuck slot | nothing: a claim on a full pool asks, in the terminal, before the TUI opens | `yi lanes reap N`, `yi doctor --fix` for the interactive case |

The claim prompt, using today's pool:

```
no free lane. left behind:
  0  yi/pid-69176  20 h  1 untracked path  taking it loses the path
  1  yi/pid-63020  18 h  clean, merged     taking it loses nothing
take 1? [Y/n/0]
```

`tty_ask` already exists at the start path (`main.rs:711`) and the claim runs
under the pool lock, so the answer is applied by the same `detach` reap uses.
A non-interactive start (`yi ask`, a drive, the daemon's worker) keeps the
refusal, which names `yi lanes`. `yi lanes reap N` stays as the shell form of
the same answer, unadvertised: the refusal and the listing point at the
prompt, not at the verb. `yi doctor --fix` keeps its row for scripts.

`--here` stays. It is the one flag, for the one case (editing the trunk's own
git state), and `/lanes` and `/land` with no title work under it as reads.

### 4.2 What the model learns

Nothing about lanes. The environment block drops the lane line and keeps the
first line, which already says the branch and the tree state:

```
cwd: /Users/…/.yi/lanes/897d…/1 (git: yi/01a06f8d, 3 modified)
```

plus, only while a landing exists, one line from memory:

```
landing: PR #191 open · lint ● test ⟳ · 3 h ago
```

The model commits. The person lands. A red job still steers the model with the
job name, and that steer is the whole protocol the model needs: read the log,
fix, commit, and say so. The phrase `land with /land "Title"` leaves the
block, because a verb the reader cannot run is noise every turn.

The one case that needs more is an autonomous goal that must ship without a
person. That is a `land` tool, the same word and the same `LaneHandle::land`
behind it, classified outward-facing (asks in ask mode, allowed in auto mode
only under a goal that says so). It is phase two, gated on a goal that
actually needs it; nothing in the repo lands a goal today.

### 4.3 What this deletes

- `/pr` and `/base` as verbs (`slash.rs:16-17`, `:164-165`, `land.rs:389-394`
  folded into `land_blocking`).
- The `needs a title` refusal (`land.rs:279-281`); no title means status.
- The reap hint in `format_lanes` and in the doctor row; the listing states
  the loss, the prompt takes the answer.
- The lane line for the model (`environment.rs:117-119`, `land.rs:243-252`);
  `describe` survives only as the quit line, in the D143 form.
- Two rows in the ARCHITECTURE feature table's verb list and the matching
  `commands.rs` slash table (`crates/tui/src/commands.rs:13` pins that the
  table covers every runtime verb, so the deletion is one edit and one test).

### 4.4 Pressure test

- Breaks: `slash_table_covers_every_runtime_verb`; the `/base` and `/pr` arms
  in `cli_surfaces` if any (none found by name); the console's verb routing at
  `acp/lib.rs:722` is a passthrough and needs nothing.
- New state: none. The prompt's answer is the reap that exists.
- Slow git or a dead forge: `/land` reports the step that stalled, as it does;
  the fetch before push rides the 120 s bound and a failed fetch skips the
  merge with a note rather than blocking the push.
- Maintains: net negative in `slash.rs` and `land.rs`; +30 in `main.rs` for
  the prompt; the D143 renderer is shared.
- The person who learned `/pr` and `/base` types them once and reads
  `/pr: try /land` and `/base: /land merges main in first`; two lines in
  `lane_verb`, removed after a release.
- ADR: **D144**, "two lane verbs: `/land` ships and reports, `/discard` throws
  away; the pool asks at claim; the model gets the tree state and the
  landing, never a verb".

## 5. Shortlist

1. **D144: two verbs.** Problem: nine words before the first landing, and one
   of them addressed to a reader who cannot use it. Shape: `/land` absorbs
   `/base` and `/pr`; the full-pool claim prompts; the model's lane line goes,
   a landing line comes. Replaces `/pr`, `/base`, `yi lanes reap N` as an
   advertised verb, and `describe` in the environment block.
2. **D143: one lane line.** As in §3.2. Prerequisite for the claim prompt (the
   prompt prints the same lines) and for the ledger join; carries the ACP
   `bind_session` fix.
3. **One branch reader.** As in §3.2. Deletion, no ADR; first, because D143
   reads HEAD per slot through it.

Order of work: the branch reader, then D143, then D144. The landing reattach
and age from §3.2 (I) ride inside D144, since `/land` with no title is the
`/pr` it replaces and must show the forge's truth on resume.

## 6. HAR review (2026-09-06)

Reviewed §3.2, §4 and §5 against `har`, `har-threat`, `har-async` and
`har-concurrent`. Findings are ordered by risk. Each names the rule, the gap in
the proposal, and the change that closes it. D143 and D144 are amended in place
in §6.2; nothing is renumbered.

### 6.1 Findings

| # | rule | gap | change |
|---|---|---|---|
| 1 | concurrent: never hold a lock across a wait a human or a peer controls | §4.1's claim prompt runs "under the pool lock". `pool.lock` is the flock every claim and release in every process takes (`mod.rs:302-307`). A person reading the prompt holds every sibling worker's start until they answer. | Release the lock, ask, re-take it, re-probe the chosen slot. The slot may have been taken meanwhile (D130 takes a clean one silently), so the answer is applied only if the slot is still an orphan with the same session name; otherwise re-list and re-ask, at most twice, then refuse with the listing. A tty that closes mid-prompt is a refusal, not a default. |
| 2 | threat: state the failure posture; an unread control must not read as a safe one | §3.2 says a slow git read degrades a field to `?`. §4.1's prompt then says "taking it loses nothing" from fields that may be `?`. A timed-out `git status` on a dirty tree renders as clean. | `SlotView::Held`/`Orphan` carry `tree: TreeState` where `enum TreeState { Known { modified: u32, untracked: u32, ahead: u32, behind: u32 }, Unread { step: &'static str } }`. The line prints `tree unread (status timed out)`; the prompt never offers an `Unread` slot as lossless and never preselects it; the control test deletes the `Unread` arm and watches the prompt offer a dirty slot as clean. |
| 3 | concurrent: one owner, one identity; a bind after a claim is check-then-write across two names | §5 fixes the daemon path by calling `bind_session` after `attach_store`. The claim still happens first under `pid-<worker pid>`, so a second `session/new` on the same worker claims under the name the first still holds, and `checkout -B` refuses a branch checked out elsewhere. Bind renames after the fact and cannot fix the second claim. | The ACP worker knows the store id before it builds (`lib.rs:296`, `session_id`, before `(self.build)` at `:303`). The claim takes that id: `SessionBuilder` gains the id, `claim_lane` uses it as `resume_id`, and `bind_session` becomes the CLI-only rename for the `yi ask` path that creates the store after the claim. Journey: two `session/new` on one worker hold two slots under two branches. |
| 4 | async: a deadline composes, a duration does not; no wait on the render thread | §3.2 gives every per-slot read the 5 s probe. Four reads per slot on three slots is 60 s worst case, on the port call that renders `/lanes` (`port.rs:213-220`). The forge reattach in §3.2 (I) on `LaneHandle::new` puts a 120 s `FORGE_TIMEOUT` on the session start path when the forge is down; `/land` with no title puts two forge calls on the port call, as `/pr` does today. | One `Instant` deadline per `Pool::list` (3 s), threaded to each `capture`; a read past it is `Unread`. The reattach runs on the land thread's shape (`land.rs:283-291`) and reaches the TUI as `LandingState`; the start path never waits on the forge. `/lanes` and `/land` with no title dispatch as `Command::SummarizeBranch` does (`app.rs:1031`) and return as a cell, so the port call returns at once. |
| 5 | har: invalid states unrepresentable; a raw string that reaches argv is validated once, at the reader | §3.2 (J) unifies the branch reader but keeps its `Option<String>` shape. `Pool::branch_of` today feeds that string to `git branch -D` and `merge-base` (`mod.rs:573-575`) unvalidated, and `None` means both "detached" and "unreadable", which `abandoned` treats as free (`mod.rs:537-539`). | The reader returns `Result<Head, LaneError>` with `enum Head { Branch(BranchName), Detached(Sha) }`; `Sha` is a 40-hex newtype parsed once (`SlotState.base` stays a string on disk; it is parsed at read). `abandoned` frees on `Detached`, refuses on `Err`. The HEAD read is bounded (`take(256)`); a `.git` file's `gitdir:` pointer is canonicalized before it is joined. |
| 6 | threat: bound every collection a peer can grow | `parse_jobs` (`land.rs:115-135`) bounds each name to 64 characters and the count to nothing. The status row draws one glyph per job; §4.2 puts `landing:` with every job name into the environment block every turn. A workflow with 300 checks is 300 glyphs on the row and 19 KB in the prompt, chosen by anyone with a workflow commit. | `JOBS_MAX = 32` at the parser; the 33rd and later collapse into one `Other("+N more")` job. Fixture: a rollup with 4 KiB of jobs renders a bounded line. |
| 7 | concurrent: at most one owner of a resource; a spawn with no guard is N pollers | `/land` twice spawns two `land_blocking` threads (`land.rs:283-291`), each with its own 2 h poll loop writing `landing`; the latch stops duplicate steers but not duplicate polls or interleaved `set_landing`. §3.2 (I) adds a third writer. | `LaneHandle` gains `polling: AtomicBool`; `land` and the reattach take it with `compare_exchange(false, true, AcqRel, Relaxed)` and clear it on exit; a second `/land` while polling reports `landing in progress`. Test: two `/land` calls, one thread. |
| 8 | har: a wire shape carries no clock; `Instant` is not serializable and not a §19 field | §3.2 (I) puts the refresh age beside `Landing`. `Landing` is `_yi/landing` on the wire and a durable struct. | The age is a TUI-local stamp: `app.rs:607` records `Instant::now()` when `LandingState` arrives, and `landing_segment` takes the elapsed time as an argument. No field on `Landing`, no change to the fixture. |
| 9 | async: a multi-step protocol names the state each failure leaves | §4.1 folds fetch and merge into `/land` before the push. A merge that stops on a conflict leaves `MERGE_HEAD`; a fetch that fails leaves a stale `origin/main`. The steps after each are unstated. | Order and posture, written into `land_blocking`: fetch fails → note, merge skipped, push proceeds (a stale base is what `/base` would have shown); merge conflicts → `git merge --abort`, verify a clean `status`, steer with at most 20 file names inside a fence, no push, landing stays `Unlanded`; abort fails → `LaneError::Git` and the steer names the lane path so the person can resolve by hand. Test: a conflicting fixture pushes nothing. |
| 10 | threat: R, an action with no receipt | The claim prompt in §4.1 deletes an untracked path and a branch on a `y`. D124 gave the warmer an action-ledger row for less. | The prompt's answer writes a D81 action-ledger row: slot, session left, branch kept or deleted, untracked count, and who answered (tty). `yi lanes reap N` writes the same row. |
| 11 | layout: a crate boundary is not free; a reader lives beside its writer | §3.2 (B) joins the ledger inside `Pool::list`. The ledger's path and loader live in `crates/acp` (`load_ledger`, `ledger_path`), which depends on `runtime`, not the reverse. `slash::run` in `runtime` cannot reach them. | `format_lanes(views, ledger: Option<&DaemonLedger>)`. `yi lanes` in the CLI loads the ledger and passes `Some`; `/lanes` in a session passes `None` and prints the pool without names. One renderer, two callers; the join moves into the session only if the ACP worker is later given the ledger by its daemon. The loader bounds the file read (`take(1 MiB)`); the rename write in D118 means a reader never sees a torn file. |
| 12 | har: integers from outside the process | Counts from `rev-list --count`, porcelain line counts, reflog epoch seconds. | `u32::from_str` / `u64::from_str`, never `as`; relative age by `saturating_sub`; a parse failure is `Unread`, not zero. Listed so it is not skipped, as in the 09-04 review. |
| 13 | threat: I, text from a foreign writer rendered under the user's eye | The ledger `name` is the session's first prompt, and the reflog and porcelain outputs are repo data. | `name` through `environment::sanitize` (charset check, 64 characters) and cut to 32 for the line; file names in the conflict steer through the same filter; nothing from git output reaches argv without `BranchName` or `Sha`. |
| 14 | threat: E, the model and the outward verbs | §4.2 removes the lane line from the model and defers a `land` tool. Correct, and recorded so it survives: while no tool exists, the model has no path to push or open, and the permission table's row for `/land` stays a person's row. The phase-two tool, if built, is classified outward-facing before it has a name. | No change; the finding is the absence, written down. |

### 6.2 Amended decisions

- **D143** gains: `TreeState::Unread` as a first-class state that never renders as clean; a single deadline per listing; the reader returns `Head`; the ledger join is the CLI's, behind `format_lanes(.., Option<&DaemonLedger>)`; `JOBS_MAX` at the forge parser.
- **D144** gains: the claim prompt asks outside the pool lock and re-verifies under it; the prompt writes an action-ledger row; `/land`'s base step states its three failure exits; one poller per handle; the reattach and the no-title status run off the port call; the ACP worker claims with its store id.
- The ordering in §5 stands: the branch reader first, because finding 5 changes its return type and D143 builds on that type.

### 6.3 Control tests, one per finding that has a control

Each is red when its control is deleted:

- a dirty slot whose `git status` times out is listed as `tree unread` and the prompt does not offer it (2)
- a second claim from another process completes while the first waits at the prompt (1)
- two `session/new` on one worker hold two slots under two branches (3)
- `Pool::list` on a wedged git returns within its deadline with `Unread` fields (4)
- a detached HEAD reads as `Head::Detached`, an unreadable HEAD as `Err`, and neither reaches `git branch -D` (5)
- a rollup of 300 checks renders 32 glyphs and one `+268 more` (6)
- two `/land` calls spawn one poller (7)
- a conflicting base pushes nothing and leaves no `MERGE_HEAD` (9)

### 6.4 Not findings

- `git_summary` and `local_time` spawn a process on the environment hook (`environment.rs:113-130`), which is called on the session's prompt path. Whether that path is an async context is not established here; the 5 s probe is bounded either way. Recorded, not owned by this plan.
- The `pid-<pid>` session name for the `yi ask` path is fine: the CLI creates its store after the claim and binds; only the daemon path (finding 3) knows the id first.
- `Landing` staying a wire enum with no `Blocked` variant for a conflicting base is deliberate: a steer carries the file list, and a new variant is a §19 change for a state that lasts one turn.
- Timing side channels, AEAD, and nonces have no instance at these boundaries; walked, not skipped.
