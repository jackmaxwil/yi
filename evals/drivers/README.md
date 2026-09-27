# evals/drivers — paid measurement runs

Stdlib Python and POSIX sh. Each driver preflights its preconditions **by name**
and refuses with the missing one; no driver ever echoes a key value.

## tbv4_baseline.sh — Terminal-Bench v4, six-task subset

The dataset is pinned by digest (`terminal-bench/terminal-bench@sha256:39d9f44b…`,
the leaderboard's `DATASET_REF`), never `@latest`. The subset is one cheap
task per domain, chosen by `expert_time_estimate_hours` and `cpus` from the
task metadata: `html-js-filter` (Security), `photonic-waveguide-routing`
(Software), `heat-pump-warranty` (Operations; it replaced `music-harmony` on
2026-09-08, tbv4-design.md §12), `bun-sourcemap-leak` (Software),
`foodstuff-beta-activity` (Science), `cargo-flight-dispatch` (Operations).
No GPU task, no multi-container task. Every v4 task gives the agent
28,800 s; `TBV4_TIMEOUT_MULT` (default and ceiling `0.125`, one hour) bounds a
trial, and the driver refuses a larger value before it checks anything else.
The model is `openrouter/z-ai/glm-5.3-flash` and nothing else
(docs/archive/plans/2026-09-06-tbv4-evals.md §8).

Preconditions, all preflighted by name:

```sh
export OPENROUTER_API_KEY=...          # env-only: crates/ai/src/auth.rs
open -a Docker                         # daemon must answer `docker info`
uv tool install harbor                 # the `harbor` CLI (0.22.0 at the time of writing)
rustup target add x86_64-unknown-linux-musl
just package-musl <version>            # zig supplies the crt; see the recipe
sh evals/drivers/tbv4_baseline.sh
```

`TBV4_TASKS`, `TBV4_ATTEMPTS` (harbor's `-k`), `TBV4_CONCURRENCY` (`-n`),
`TBV4_RUNS_DIR` (default `runs/tbv4`, the directory harbor is told to write
with `-o`) and the caps `TBV4_SOFT_CAP` / `TBV4_HARD_CAP` override. The
driver exports `EVAL_SUITE_REV` (the digest) and `EVAL_TIMEOUT_MULT` so the
adapter's config fingerprint names both: a one-hour row and a full-length
row never share one.

Caps: soft $20 (refuses to start another task), hard $25 (stops the stream),
both read from the summed `usage.cost.total` of every synced `yi.jsonl` by
`tb21_cost.py`. A cap stop exits **2**, a finished suite exits 0, a missing
precondition exits 1.
The cap comparison lives in `tb21_cost.py --soft/--hard`, and its **exit
code** is the gate: a spend that cannot be computed raises out of the probe,
and any non-zero probe exit stops the stream. "Cannot be computed" includes a
run carrying a D79 `usage.unknown` turn, and — since the TB2.1 driver read
`runs/` while harbor wrote `jobs/`, so the probe summed nothing and the cap
never tripped — a runs directory holding fewer transcripts than tasks run:
the driver passes `--min-files <tasks so far>` and the probe exits 2 when a
task left no `yi.jsonl` behind. `evals/selftest.py` (`check_cost_cap`,
`check_driver_ceiling`) holds those lines.
A timeout is a result, never a retry; attempts are harbor's, never a
runner-level retry.

The trials land under `runs/tbv4/<job>/<task>__<id>/` with `result.json`,
`agent/yi.jsonl`, `agent/yi/sessions/*.jsonl` and the telemetry sidecar the
adapter turns on at install. `evals/axes.py` (plan S2) reads that directory.

## tbv4_sweep.sh — one continuous run over a task list, held by watch.py

For calibration: which v4 tasks can show a harness change at this model. Same
dataset digest, model, binary and preflight as the baseline (the multiplier
ceiling is checked first), but every task in `TBV4_LIST` (default
`tbv4_sweep_tasks.txt`, 48 host-feasible tasks in a seeded shuffle: no GPU task,
no 16-CPU task, not the six baseline tasks) goes to one `harbor run`, so all
`TBV4_CONCURRENCY` (default 3) slots stay full until the wall instead of waiting
on each task's slowest trial. What finishes before the wall is the sample; the
shuffle keeps the unfinished tail random.

`watch.py` owns the harbor process (its own process group) and polls every 60 s:
a trial past $1 or 180 turns has its containers stopped; the run's spend past
`TBV4_HARD_CAP` (default $20), the wall past `TBV4_WALL` (default 27,900 s,
7 h 45 min) or host space under 8 GB (macOS's figure for important usage, which
counts Time Machine local snapshots as free; 3 GB plain `df` as a floor) stops the
group and this run's trial containers, exits 2 and writes the reason to
`<runs>.STOPPED`. It also removes terminal-bench images no container uses:
harbor's `--rmi local` never removes a pulled image, and these are pulled by
digest, untagged, so they go by id after two idle polls. `evals/selftest.py` (`check_driver_ceiling`,
`check_watch_stops`) holds the ceiling and the stop path.

```sh
TBV4_RUNS_DIR=runs/tbv4-sweep sh evals/drivers/tbv4_sweep.sh
```

## Runner mode — the protocol levers.py calls

```sh
EVAL_RUN_ID=<run-id> sh evals/drivers/tbv4_sweep.sh --runner <overrides.json> <task>...
```

Same preflight, dataset, model and binary as the sweep. The tasks run as one harbor job under
`watch.py` with a hard cap from `trials.py caps` (the smaller of what the stage's $10 and the
week's $30 leave; the call is refused outright when its predicted cost, tasks x $0.13, would pass
the stage's $8 or the week's $25 soft cap). `{}` overrides run the defaults; any other file rides
into the trial as `YI_LEVERS` (E16). Sessions land under `~/Development/yi-runs/<run-id>/`, one
row per trial is appended to `evals/trials/<run-id>.jsonl` (trials/README.md) and printed on
stdout, and harbor's own output goes to stderr. A trial the watcher stops past $1 or 180 turns
is marked `censored`; an unpriced trial counts $1 toward every cap.

## T1 frontier — no targets, $0 spent (campaign 3)

T1 pins its two targets on a baseline row's failing tasks. Row `0001` is
`fixtures@375d9b1`, **3/3 passed** — the failing set is empty, so there is
nothing to pin.

The suite explains the saturation: the three fixture tasks pin runner mechanics,
not task solving (`evals/README.md`, "Task runner"). Their content is echo a
marker (`answer-echo`), leave no stray file behind (`clean-workspace`), write one
line into one file (`edit-file`). Each can be failed on merit — a littered
workspace fails `clean-workspace` for a real agent reason — but none is hard, and
`dryReward` records what faux does rather than difficulty. A suite with no hard
task has no frontier to find.

Two blockers, then, and only one of them is money. **(1)** No hard suite: TB2.1
above is the one that can be failed on merit, and its four preconditions are red.
**(2)** No per-task history: `run.py` prints its per-task JSON rows to stdout and
nothing persists them, while the ledger row is an aggregate — so even a suite
with failures leaves a later campaign unable to read which tasks failed without
re-running the suite it is supposed to be picking from.

3/3 on three trivial tasks is absent evidence of frontier headroom, never the
negative result `J11`'s kill test asks for.

## ARC-AGI-3 — unblocked; the driver is `evals/arc/`

The key exists and the adapter is built, so this section is no longer a plan.
See `evals/arc/README.md` for the mechanism and `docs/eval-ledger.md` rows
`0010`/`0011` for the first two measured runs. Zero-budget connectivity smoke,
unchanged and still worth running first — note that no `.env` is copied into the
scaffold, the key is passed through the environment:

```sh
cd ref/benchmarks/ARC-AGI-3-Agents && uv sync
ARC_API_KEY=... uv run main.py --agent=random --game=ls20
```

The paid run is `evals/arc/yi_arc.py`, which registers a `Yi` subclass of the
scaffold's `Agent` and drives one `yi ask --continue` process per ARC action.
`ARC_COST_CAP` stops the stream the way `tb21_cost.py` does, except that here
the spend is summed from the same `yi_usage.parse_events` the harbor and pier
adapters use, so a D79 `usage.unknown` turn cannot price at zero.

Unlike TB2.1 there is no docker, no musl target and no harness install: the ARC
service hosts the game environments, so the only spend is model tokens. The
binding budget is wall clock, not USD — a Yi turn takes ~10s against a random
agent's ~9 actions/second, and 150 actions is ~24 minutes for ~$0.52.

## cache_probe.sh — two turns per OpenRouter model, cache read back

`yi ask` twice per model into a fresh `--session-dir`, the second with
`--continue`, then `cache_probe.py` prints each turn's `input / cacheRead /
cacheWrite / cost` and hit rate, and the session's `yi stats --json` cache
block. Exit 2 when a warm turn read nothing back or any turn was `usage.unknown`;
1 on a missing precondition. Three one-line turns per model cost well under a
cent, so there is no cap.

```sh
export OPENROUTER_API_KEY=...          # env-only: crates/ai/src/auth.rs:19
cargo build -p yi-cli
sh evals/drivers/cache_probe.sh        # CACHE_PROBE_MODELS / EVAL_BINARY / CACHE_PROBE_RUNS override
```
