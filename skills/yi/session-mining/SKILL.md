---
name: session-mining
description: >
  Sweep past Yi session logs for repeated pain points — tool failure streaks,
  permission denials, retries, aborts, pivots, orientation cost, delegation
  cost — with a versioned extractor, and judge the printed report into
  human-gated proposals. Use when asked to mine sessions, find agent pain
  points, or analyze past runs. User-run only, never scheduled. Never applies
  fixes; it reports.
trigger: mine sessions, session mining, pain points, extract.py
---

# Session mining

Yi sessions are typed JSONL under `~/.yi/sessions/<cwd-slug>/` (one file per
session, one JSON object per line). They carry structured signals far stronger
than prose: tool results with error flags, permission denials with evidence,
retries, aborts, compactions, `ext_record` telemetry.

The split this skill runs on:

| deterministic (`extract.py`, exact code) | model-judged (you, gated by the user) |
|---|---|
| what counts as a failure, a repeat, a streak, a denial | what a cluster *means* |
| the fingerprint that groups failures into one issue | which issue is worth a fix |
| issue state (NEW/CASED/FIXED/REGRESSED/RETIRED) | what the fix should be |
| redaction of every emitted string | the wording of a proposal |
| novelty score, backtest count | whether the proposal survives them |

Nothing in the left column is a judgement call, and nothing in the right column
lands without the user. The extractor never mutates lifecycle state on its own:
marks are written only by a user-run `--mark`.

## Method

### 1. Sweep with the extractor, never by hand

Run the versioned extractor that sits beside this skill, in the `ipython`
kernel (or any shell). Hand-rolled sweep code is not an option: it skips the
redaction rules, the schema, and the determinism the report's readers assume.

```python
import subprocess
print(subprocess.run(
    ["python3", "skills/yi/session-mining/extract.py"],
    capture_output=True, text=True, check=True).stdout)
```

Defaults: the corpus is the session directory for the current working
directory; derived stores land in `.yi/mining/` (self-ignoring, disposable,
regenerated every sweep). `--sessions DIR` mines another project, `--out DIR`
moves the stores.

Raw JSONL is immutable truth, the extractor is versioned, and the derived
stores are caches — delete `.yi/mining/` whenever it looks wrong and sweep
again. `marks.jsonl` is the exception: it is the human decision log, append
only, and the join happens at derive time.

### 2. Read the report, not the corpus

Only the printed report enters context; thousands of entries never do. It
arrives in a fixed order:

1. **coverage line** — `N sessions scanned, M skipped (reason), covering
   DATE..DATE`, plus corrupt lines skipped and `K of N carried a tool call`.
   Mandatory in anything you paste onward: a cluster count without its
   denominator is not evidence, and `K` is the denominator for every
   behavioural rate — quoting `N` counts zero-tool faux runs as wins.
2. **redaction caveat** — the entropy rule masks 32+ char high-entropy tokens,
   so SHAs and ulids read as `[MASKED]` in examples.
3. **signal census** — counts by entry type, message role, tool, tool error,
   and `customType`. Read this first: it says what the corpus can support
   before any rule is designed. A signal with a count of zero cannot carry a
   proposal, however good the idea is.
4. **top issues** — fingerprint, tool, count, lifecycle state, first/last seen,
   redacted example.
5. **orientation and delegation summaries**, wins, repeated calls, compactions.
   Orientation ends at the first call that can change the tree; a `bash` call is
   screened by its command through the `builtins.rs` read-only vocabulary, and
   `ipython` counts as a mutation because arbitrary code cannot be screened.
6. **extractor and schema version footer.**

The derived stores hold the rest (`mu.jsonl`, `issues.jsonl`,
`orientation.jsonl`, `delegation.jsonl`) for when a specific question needs
them — query them in the kernel, not by pasting them into context.

### 3. Judge clusters into proposals, ranked by the outcome ladder

**eval case > tool/affordance TODOS row > doctrine or prompt edit > fitted
constant > trigger rule.**

An eval case — a deterministic replayable scenario with a pass condition, seen
red before it is claimed fixed — is the preferred unit of learning: one
occurrence suffices, it does not rot, and its pass/fail is external. A rule is
last because it needs repetition to pay and decays as the repo moves. For a
rendering or shim defect the eval case is the scrubbed session itself under
crates/tui/tests/fixtures/sessions/, replayed through the reducer.

State each proposal as: the cluster (fingerprint + count + coverage), the
smallest change that would have prevented it, and where it lands.

### 4. Gate every proposal before you state it

- **Novelty.** Every doctrine, prompt, or rule proposal runs
  `python3 skills/yi/session-mining/extract.py --dedupe "<the sentence>"` and
  cites its score. At or above 0.60 the text already exists: drop it, or say
  which existing line failed and why repeating it would work this time.
  Sources that are absent are named in the output — a missing source is not a
  passing score.
- **Backtest.** Every trigger rule ships with
  `--backtest "<regex>"`: how many times it would have fired, and where. A rule
  that fires in most sessions is noise — tighten it or drop it. Read the count
  as a firing rate and nothing more: it is confounded, because the sessions it
  fires in are the same sessions whose outcomes you are reasoning about, and it
  cannot show the reminder would have changed any of them. It rejects noisy
  rules; it never confirms a good one.
- **Corpus support.** A proposal whose signal the census does not count is a
  hypothesis, and is labelled as one.

### 5. Propose, never apply

The user lands changes. A rule they accept becomes a file in `.yi/rules`, which
is also where `/advisor promote <advice-id>` lands one they agreed with live.
Never edit config, rules, prompts, or skills yourself, and never write a mark
on the user's behalf.

Once an issue is acted on, the user records it — the extractor's lifecycle is
driven only by these:

```sh
python3 skills/yi/session-mining/extract.py --mark <fp> cased --ref docs/cases/<case>.md
python3 skills/yi/session-mining/extract.py --mark <fp> fixed --ref <commit>
python3 skills/yi/session-mining/extract.py --mark <fp> retired
```

`REGRESSED` is derived, not marked: a fixed fingerprint reappearing in a
session newer than its fix mark. That is the board's whole point — it can tell
you that a fix did not hold.

## Redaction

Every emitted string — μ rows, issue examples, orientation and delegation rows,
the printed report — is redacted at extraction time: credential-store path
lines dropped entirely, `authorization|bearer|api key|token|secret|password`
values masked, long mixed-class `NAME=value` masked, high-entropy 32+ char
tokens masked, `$HOME` collapsed to `~`.

Never weaken these rules to make an example more readable, and never route
around the extractor to get an unredacted quote. Anything leaving the machine —
a ledger row, an eval case, a pasted report — goes through it.

`python3 skills/yi/session-mining/extract.py --selfcheck` is the gate: it
sweeps a committed fixture carrying planted fake secrets and fails if any plant
reaches an output file, if two sweeps differ by a byte, or if the lifecycle and
novelty logic drift. Run it after any change to the extractor.
