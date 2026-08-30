# evals/drivers — paid measurement runs

Stdlib Python and POSIX sh. Each driver preflights its preconditions **by name**
and refuses with the missing one; no driver ever echoes a key value.

## tb21_baseline.sh — Terminal-Bench 2.1, six-task subset

Subset spans difficulty at the ≤900s agent timeout: `overfull-hbox` (easy, 360s),
`fix-git` (easy), `regex-log` (medium), `db-wal-recovery` (medium),
`password-recovery` (hard), `write-compressor` (hard). Everything ≥1200s is
excluded — wall clock, not USD, is the binding budget at these prices.

Four preconditions, all red on this machine as of run 0001:

```sh
export OPENROUTER_API_KEY=...          # env-only: crates/ai/src/auth.rs:19
open -a Docker                         # daemon must answer `docker info`
pip install harbor                     # the `harbor` CLI
just package-musl <version>            # needs `rustup target add x86_64-unknown-linux-musl`
sh evals/drivers/tb21_baseline.sh
```

Caps: soft $20 (refuses to start another task), hard $25 (stops the stream),
both read from the summed `usage.cost.total` of every synced `yi.jsonl` by
`tb21_cost.py`. Override with `TB21_SOFT_CAP` / `TB21_HARD_CAP` / `TB21_RUNS_DIR`.
A cap stop exits **2**, a finished suite exits 0, a missing precondition exits 1
— the three outcomes a caller must tell apart, since a suite that burned its
slice and a suite that ran to the end used to share exit 0.
The cap comparison lives in `tb21_cost.py --soft/--hard`, not in the shell, and
its **exit code** is the gate: the shell used to compare the probe's stdout, so
a probe that failed left the spend empty and the failed comparison read as
under-cap — the one automated guard on the slice, failing open. A spend that
cannot be computed now raises out of the probe, and any non-zero probe exit
stops the stream. `evals/selftest.py` (`check_cost_cap`) holds that line.
"Cannot be computed" includes a run carrying a D79 `usage.unknown` turn: a
stream that died before its usage chunk prices at zero, so summing it would let
an unmeasurable run walk under the cap forever. `parse_events` reports those as
`costUnknownTurns` and refuses a `costUsd`; the probe stops on them, the ledger
row's cost cell reads `?` rather than `-`, and
`check_unknown_usage_is_not_a_free_turn` holds that line.
A timeout is a result, never a retry; an image that fails to build gets one
rebuild and is then recorded as an environment failure in the row's notes.

The same driver runs a **campaign slice**: `TB21_TASKS` pins the task list and
`TB21_ATTEMPTS` sets harbor's `--n-attempts`, so a T1 frontier pair at k=2 is

```sh
TB21_TASKS="password-recovery write-compressor" TB21_ATTEMPTS=2 \
    sh evals/drivers/tb21_baseline.sh
```

Attempts are harbor's, never a runner-level retry: a timeout inside an attempt
still stands as a result.

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

## ARC-AGI-3 — blocked, no driver

`ARC_API_KEY` needs an arcprize.org platform account (a human signup). The
scaffold smoke, once a key exists, is one command in the ref clone and spends
no LLM budget:

```sh
cd ref/benchmarks/ARC-AGI-3-Agents && cp .env.example .env
ARC_API_KEY=... uv run main.py --agent=random --game=ls20
```

The Yi attempt itself is not a measurement run: it needs a custom agent bridging
the scaffold's `FrameData` loop to `yi ask`, which is build work.
