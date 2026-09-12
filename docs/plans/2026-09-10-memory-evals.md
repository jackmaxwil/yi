# Memory evals: episodes of live Yi sessions that need what an earlier session learned

```
status:  proposed 2026-09-10, second draft. Our own suite in evals/, run by
         yi's own harness; nothing from AMB is run. Nothing here has run yet.
tree:    memory S1 at 36de8ca (PR #374, D169). This is the measurement
         docs/plans/2026-09-10-memory.md gates S2 and S3 on.
model:   openrouter/z-ai/glm-5.3-flash, the ledger's live model (rows
         0020-0025), one frozen yi version per measurement.
issues:  #373 (S1). A measurement issue under "Evals on the ledger" before the
         first paid episode (D106).
inspiration: vectorize-io/agent-memory-benchmark's sdebench (a regression whose
         obvious fix fails a hidden test, and only a past decision gives the
         right one; first-try solves and interventions as the metrics) and
         vectorize-io/hindsight (capture everything, retrieve by the question,
         push it in). Ideas only; no code, data or model choice is taken.
lineage: docs/plans/2026-09-06-tbv4-evals.md (the instrument, the ledger,
         evals/run.py); docs/plans/2026-09-10-memory.md.
```

## The questions

1. **Write.** When a session contains a correction, an incident or a stated
   preference that a later session will need, does Yi save a note that
   carries it, and does it stay quiet in sessions that contain none?
2. **Pull.** When that note sits in the index, does a later session open it
   and act on it?
3. **Against the alternatives.** Three other places the fact could live:
   - nowhere;
   - the repo's `AGENTS.md`, which yi already loads;
   - a Hindsight-shaped path that keeps every transcript and pushes back what
     matches the new task.

   How does each compare with notes on pass rate, cost and noise?

## The unit: an episode

sdebench seeds the past once and disables write-back, so it cannot measure a
note written in one session helping a later one. That later-session case is
the one Yi's memory exists for, so ours is built around it.

An **episode** is an ordered list of live `yi ask` sessions in one repository
directory under one HOME:

```
teach  →  gap₁ … gapₙ  →  test
```

- **teach.** A real coding task in which the fact arrives the way the header
  says a note is earned: a scripted user correction (a `--session` follow-up
  after the first answer), an incident the task itself surfaces (a test that
  fails for a reason the code does not explain), or a preference stated up
  front.
- **gap.** Routine tasks in the same repository with nothing worth keeping.
  They churn the index, cost save slots, and measure the write policy's noise.
- **test.** A fresh session. Its task has an obvious fix that the fact rules
  out. The fact is in no file, no commit message and no comment the test
  session can read. `reward.sh` runs a hidden check that the obvious fix
  fails and a fact-respecting fix passes. The pytest tail is fed back, as
  sdebench does, up to two interventions.

Every user turn is scripted text, so there is no simulated user and no LLM
judge. Every grade is an exit code or a string fact read from the session
JSONL.

### What a fact looks like

Six kinds, chosen because none can be derived from the code in front of the
agent:

| kind | taught by | example fact | the test session's trap |
|---|---|---|---|
| compatibility | correction | "keep accepting the v1 `ts` field; the mobile client still sends it" | a cleanup task where deleting the v1 branch is the clean fix |
| rejected approach | incident | "`re` here was 40× slower on real inputs; the loop is deliberate" | a readability task that invites the regex |
| environment | incident | "CI runs as root, so a chmod-based permission test passes locally and lies" | a new permission test the obvious way |
| protocol | correction | "timestamps on the wire are integer milliseconds, never ISO strings" | a new endpoint serialising a `datetime` |
| tool gotcha | incident | "`tool --json` prints progress on stderr first; parse from the first `[`" | a new parser over the same tool's output |
| preference | stated | "no new dependencies; stdlib only" | a task where a small library is the obvious answer |

Each episode's fixture names its **anchors**, the strings a note carrying the
fact would contain (`ts`, `v1`, `milliseconds`), and its **cause file**. That
is how saves and reads are graded without a judge.

### Fixture layout

The layout follows `evals/fixtures/tasks/`; the runner is a sibling,
`evals/episodes.py`:

```
evals/fixtures/episodes/<id>/
  episode.json      id, kind, anchors, cause_file, timeoutSec, sessions: [
                      {role: teach, prompt, followups: [...]},
                      {role: gap, prompt}, …,
                      {role: test, prompt, told: "<one sentence of the fact>"} ]
  repo/             seed, a git repo with one squashed commit (no history to mine)
  note.md           the golden note, in the S1 template (the seeded arm)
  agents.md         the fact as a standing instruction (the AGENTS.md arm)
  reward.sh         visible + hidden checks for the test session
  reward_teach.sh   optional: the teach task's own check (recorded, never gating)
```

## Arms

Same model, same frozen binary, same fixtures. Only where the fact lives changes.

| arm | HOME across sessions | where the fact is at test time | isolates |
|---|---|---|---|
| **A0** amnesia | fresh for the test session | nowhere | the floor: the trap works |
| **A1** lived | one HOME for the whole episode | wherever Yi's own saves put it, or nowhere | **write + pull**, end to end |
| **A2** seeded | teach and gaps in a throwaway HOME; the test HOME gets `note.md` plus the episode's decoy notes | the index, as a golden note among decoys | **pull alone** |
| **A3** told | fresh | the test prompt's last sentence | the ceiling: the fact reaches the model for sure |
| **A4** AGENTS.md | fresh | `agents.md` committed into the repo, so project instructions carry it | the user's natural alternative: a standing instruction |
| **A5** transcript push | fresh, plus the prior sessions' JSONL | the top BM25 chunks of every earlier transcript for the test prompt, 4 096 tokens, prepended as "from earlier sessions" | the Hindsight shape without Hindsight: capture everything, retrieve by the question, push |

A5 is the architectural comparison, run in-house and deterministically. There
is no Postgres, no extraction LLM and no reranker, so it is a lower bound on
what a Hindsight-class system gets from the same transcripts. It is the right
comparison for a question about where the fact should live, not which vendor
retrieves best.

A2's decoys are other episodes' golden notes. The index then holds 20-40 real
notes on the same repository, and hook selection is tested at a realistic
size.

## Runner

`evals/episodes.py` reuses `evals/run.py`'s pieces: `yi_usage.parse_events`,
`final_answer`, `score`, and the `yi ask --json --here --yolo --deadline`
command. It changes three things.

1. **One fixed workspace per episode run.** `evals/run.py` copies each task
   into a fresh temporary directory. That would give every session a different
   memory directory, because the key is the canonical repo path. The episode
   runner copies `repo/` once to `<run>/<id>-<arm>-<k>/repo` and runs every
   session there.
2. **One HOME per episode run**, created by the runner, never the caller's and
   never shared across arms or repeats. A2 writes its notes with
   `yi memory import`. A4 commits `agents.md` as `AGENTS.md`. A5 builds its
   push block from the sessions directory with a stdlib BM25, so the file stays
   dependency-free like the rest of `evals/`.
3. **Sessions in order.** A teach follow-up is `yi ask --session <id>`. The
   test session's interventions are follow-ups carrying
   `[Feedback #n] <pytest tail>`, capped at two.

After each episode it writes `row.json`. Every field is a fact read from the
event stream or the session JSONL:

| field | read from |
|---|---|
| `test_first_try`, `test_pass`, `interventions` | `reward.sh` exit codes per round |
| `teach_saves`, `teach_saved_fact` | `memory.save` calls in teach sessions; a saved note contains ≥ 2 anchors |
| `gap_saves` | `memory.save` calls in gap sessions (the noise of the write policy) |
| `test_reads`, `test_read_fact`, `read_turn` | `memory.read` calls in the test session; the target is the saved or seeded fact note |
| `fact_in_head` | the fact note was among the loaded lines of the test session's block |
| `cost`, `tokens`, `turns`, `wall`, `spiral` | `yi_usage` as in the ledger; `spiral` = a reasoning cut or a length stop, so an upstream episode is visible rather than scored as a memory result |

## Metrics and pre-registered decisions

Per arm: test pass at first try and within two interventions, mean
interventions, cost per episode, all as the median over k runs. Memory
specific: `teach_saved_fact` rate (A1), `gap_saves` per gap session (A1),
`test_read_fact` rate (A1, A2). An episode with `spiral` set in the test
session is rerun once, and reported as inconclusive if it recurs. It never
counts as a memory result.

| if | then |
|---|---|
| A0 passes ≥ 30 % of test sessions at first try | the traps leak; fix the fixtures before reading anything else |
| A3 passes < 80 % | the model cannot use the fact even when told; the episode is too hard for this model and is cut, not counted |
| A2 reads the fact note on ≥ 70 % of test sessions and A2 ≥ A3 − 10 points | pull works; S2 does not open on this evidence |
| A2 reads the fact note on < 50 % and A3 ≫ A2 | the model does not pull. **S2 opens.** Its A/B is A2 against A2 with anchor recall on the test prompt, which is why every test prompt names the cause file |
| A1 `teach_saved_fact` < 50 % | writing is the bottleneck, not recall. Next candidates, in order: a deterministic session-end nudge, the user's `/memory save`, and last, automatic capture (A5's shape), which is the user's call because it breaks the deterministic-trigger rule |
| A1 `gap_saves` > 0.3 per gap session | the header over-invites saving; tighten its wording before any trigger work |
| A4 ≈ A3 and A4 ≫ A2 | an instruction file beats a note for these facts; the note's place is the facts a user would not write into AGENTS.md, and the fixtures should say which |
| A5 ≥ A1 + 15 points at ≤ 2× A1's cost | capture-and-push wins on this model; the architecture question reopens for a pluggable backend behind the same three verbs |
| A1 ≥ A5 − 5 points | the note design holds against capture-everything, at a fraction of the prompt |

## Size, cost and order

Estimates from the ledger, not measurements. The fixture suite on this model
costs $0.05-0.13 for 21 tasks and takes 15-45 minutes (rows 0020-0024), about
45-130 s per session.

- **Pilot:** 6 episodes, one per kind, 1 teach + 2 gaps + 1 test each.
  - 6 arms × k = 1 is 36 episode runs, about 144 sessions.
  - Under $1, about 3-5 hours sequential.
- **Slice:** 18 episodes, three per kind.
  - 6 arms × k = 3 is 324 episode runs, about 1 300 sessions.
  - Around $5-10; about 25-45 hours sequential, so it runs four wide.
- **Order:**
  1. One paid episode end to end, A1.
  2. The pilot's A0 and A3, to prove the traps and the ceiling before any
     memory arm is read.
  3. The rest of the pilot.
  4. The slice.

  One frozen version per measurement. A defect found mid-run gets a later row,
  not a patch.

Every run is a ledger row with suite `episodes@<rev>`, config fingerprint
from `yi_usage`, and the memory fields in a new `memory` column on the right:
`saved-fact / gap-saves / read-fact`.

## Building the fixtures

Episodes are hand-written, one reviewed PR per kind. They are fixtures first:
each lands with its A0 and A3 dry expectations before any live run.

- The trap is proved by two committed reference solutions per episode,
  `solutions/obvious.patch` (must fail `reward.sh`) and
  `solutions/respecting.patch` (must pass). `evals/selftest.py` applies both
  and checks the exit codes, no model involved.
- The fact appears in no repo file, commit message or comment; the self-test
  greps `repo/` for every anchor and fails if one is there.
- The test prompt names the cause file, so S2's anchor recall has something to
  fire on when that arm exists, and the A2-with-anchors A/B needs no new
  fixtures.
- The teach session's fact arrives in the scripted user turn, never in the
  repo, so a note is the only way it survives.

## Rejected

- **Running AMB or sdebench.** The benchmark measures passive ingestion,
  except sdebench, which seeds the past once. Ours needs write-back across
  sessions, and our own harness, ledger and model.
- **An LLM judge or a simulated user.** Every grade is an exit code or a
  string in the JSONL, and every user turn is fixed text. That is the
  deterministic-signal rule, and it keeps a rerun comparable.
- **Running Hindsight itself as an arm.** It needs Postgres, an extraction LLM
  and a reranker, adds a second model to the comparison, and answers "which
  vendor". A5 answers "where should the fact live" with no new service.
- **Scoring teach-session pass as a memory result.** It is recorded because a
  failed teach can change what gets saved, but memory is judged only at the
  test session.

## Open

- **The six kinds are a first guess.** The case study's fifty notes are
  mostly environment, tool-gotcha and process facts about the estate, not the
  code. Two of the six kinds may deserve to be estate facts, such as a host
  with RAM-backed `/tmp`, if the fixtures can grade them without an estate.
- **A5's push is prepended to the test prompt**, so it sits in the user turn,
  not the yard. That favours A5 slightly over a system-prompt injection;
  noted, not corrected.
- **The upstream's GLM spiral episodes** (see the ledger's glm-flash notes)
  can swamp a small pilot; the `spiral` field and the rerun-once rule exist
  for them.
