# evals/arc — ARC-AGI-3 on the official scaffold

`yi_arc.py` is a stdlib-only bridge between arcprize's `ARC-AGI-3-Agents`
scaffold and the real `yi` binary. Nothing is written into the ref/ clone: the
driver puts the scaffold on `sys.path`, registers a `Yi` subclass of the
scaffold's `Agent` into `AVAILABLE_AGENTS`, and drives `Swarm` itself instead of
shelling through `main.py` — which is also how it gets the closed scorecard back
as an object rather than out of a log file.

## Mechanism

One Yi session per game. Each ARC action is one `yi ask --json --yolo` process:
the first opens the session, every later one resumes it with `--continue`
against a per-game `--session-dir`, so **Yi's own context management is what
carries the game state across the turn loop**. The board goes in as a 64x64 grid
of hex digits plus a one-line delta ("cells_changed_by_last_action"), and the
reply is gated by `--schema` whose `action` enum is rebuilt each turn from the
frame's own `available_actions` — a non-conforming answer is exit 3, not prose
Yi has to be trusted to have meant.

`--continue` rather than a fresh session per action is the whole point: the
model's notes to itself are its earlier replies, and compaction under a growing
board history is the mechanism under measurement, not an accident to design
around.

Usage and cost come from the same event streams, parsed by
`evals/adapters/yi_usage.py` — one definition of the camelCase usage parse and
of D79's `usage.unknown` refusal, shared with the harbor and pier adapters.

## What a kernel-resident agent would add

This adapter is deliberately the *smallest* real mechanism, and it is not the
ARC-class engine the plan (§11) describes. Yi's kernel/`rlm` holds a persistent
IPython process; the ARC-class agent keeps the board as a **variable** there,
writes candidate rules as **executable predicates**, and verifies them against
the frame history in-process — spending Python, not tokens, on the search. Here
every hypothesis is re-derived in prose from a re-serialized grid each turn, and
the only memory is the transcript.

Run `0011` measured the gap instead of assuming it. On the click game the model
reached for the kernel **unprompted, 9 times in 80 turns** — and every one of
those calls opened by hand-transcribing the board back out of the prompt into
triple-quoted Python (`b57 = """7777...`), two of them ending in
`print(len(b57[0]), len(b58[0]))` because the transcription had drifted in
length. The kernel is reachable and the model wants it; it is fed prose to
re-type. So the missing pieces are concrete: **the harness should populate the
board as a kernel variable** before the turn, keep the frame history queryable
in-process, let rules land as predicates checked against that history, and give
`rlm.run` the sub-search. None of that needs a new Rust surface — it needs the
adapter to write into the kernel instead of into the prompt.

## Running

`ARC_API_KEY` and `OPENROUTER_API_KEY` are environment-only — no `.env` is
written into the scaffold, and no key is ever echoed. Install the scaffold's own
deps once (`cd ref/benchmarks/ARC-AGI-3-Agents && uv sync`), then:

```sh
cd /path/to/repo
export ARC_API_KEY=...  OPENROUTER_API_KEY=...
ARC_MAX_ACTIONS=150 ARC_COST_CAP=6.0 ARC_RUN_DIR=evals/arc/runs/arc0001 \
    ref/benchmarks/ARC-AGI-3-Agents/.venv/bin/python evals/arc/yi_arc.py --game ls20
```

Knobs, environment-only and deliberately not named `YI_*` — they are the
harness's variables, not the binary's env surface: `ARC_YI_BINARY`,
`ARC_YI_MODEL`, `ARC_MAX_ACTIONS`, `ARC_COST_CAP`, `ARC_TURN_TIMEOUT`,
`ARC_RUN_DIR`, `ARC_SCAFFOLD`.

`python3 evals/arc/yi_arc.py --selftest` needs no key, no scaffold and no
network: it pins the grid encoding, the JSON extraction, the RHAE formula, and
the refusal to price a turn whose usage never arrived.

## Artifacts

Per run under `ARC_RUN_DIR`: `summary.json` (the row's numbers plus the full
scorecard), `turns/<game>.<nnnn>.jsonl` (every Yi event stream, one file per
action — the replay), `sessions/<game>/` (the v4 session files), `run.log`. The
scaffold's own action replay lands in
`ref/benchmarks/ARC-AGI-3-Agents/recordings/`, named in `summary.json`.

## RHAE

`(human actions / agent actions)^2`, capped at 1.15, and **zero for a level the
agent did not clear**. The human baseline is not a guess: the closed scorecard
carries `level_baseline_actions` per level, and `level_actions` is what the
agent spent. `ls20`'s baselines are `[22, 123, 73, 84, 96, 192, 186]` across its
seven levels.

## Two things the scaffold does not survive on its own

A Yi turn takes tens of seconds, so the ARC socket idles far longer than between
a random agent's ~9 actions/second. Both were real failures of the first run,
not anticipated ones:

- `RemoteEnvironmentWrapper.step` catches the dropped-keepalive `RequestException`
  and returns `None`, which the scaffold's `_convert_raw_frame_data` then raises
  on — inside a daemon thread, so the run half-dies quietly. Fixed at the source
  (`Connection: close` on both sessions), with a counted retry as backstop;
  `arcStepRetries` in `summary.json` is nonzero if it ever fired.
- `close_scorecard` died the same way, which would discard a paid run's only
  score. It retries three times.
