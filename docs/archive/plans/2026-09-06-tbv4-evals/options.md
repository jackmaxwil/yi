# Three designs, one recommendation, and what the first paid run costs

Price basis, from ledger rows the catalog cross-checked to 0 %:
`openrouter/z-ai/glm-5.3-flash` at $0.075 / M input, $0.25 / M output,
$0.015 / M cached (rows 0001, 0010, 0012). Wall basis: row 0010, one
`--continue` turn ≈ 10 s on flash; the v4 reference row, 4,202.8 s per
trial on a frontier model (`tbv4-design.md` §6).

Per-trial envelope on flash, for a v4 task run to a one-hour agent timeout:
up to ~200 turns, context climbing to ~150 k before compaction, ~90 %
cached after the first turns (row 0010: 98.6 % cache reads over 151 turns).
Worst case ≈ 2 M uncached input ($0.15) + 25 M cached ($0.38) + 0.5 M output
($0.13) ≈ **$0.65 per trial-hour**, and most trials end well before the
hour. A trial that runs the full eight hours would be ≈ $5 on flash; on a
frontier model the reference row says $22.

## (a) Terminal-Bench v4 directly, harbor adapter, pinned subset

**What it is.** `harbor run --agent yi_harbor.agent:Yi -d terminal-bench/terminal-bench@sha256:39d9f44b… -i <task> … -k <n> -o runs/tbv4` over a
pinned subset, one docker environment per task, the task's own verifier.

**Subset.** Six tasks, one per domain where a cheap one exists, chosen by
`expert_time_estimate_hours` and `cpus` from every `task.toml` (no task
body read):

| task | domain / subdomain | expert h | cpus |
|---|---|---|---|
| `html-js-filter` | Security / AppSec | 0.75 | 2 |
| `photonic-waveguide-routing` | Software / Algorithms | 0.75 | 2 |
| `music-harmony` | Media / Music | 1.0 | 2 |
| `bun-sourcemap-leak` | Software / Systems | 1.5 | 2 |
| `foodstuff-beta-activity` | Science / Chemistry | 1.5 | 2 |
| `cargo-flight-dispatch` | Operations / Logistics | 2.5 | 2 |

No GPU task, nothing above two cpus, no `docker-compose` environment (11 of
the 66 tasks are multi-container; `medical-claims-processing`, the cheaper
Operations task by expert hours, is one of them and runs a Playwright
sidecar, so Logistics stands in), five of the seven domains. Every one
gives the agent 28,800 s; the runner's `--agent-timeout-multiplier 0.125`
bounds a trial to one hour, and the ledger row says so (E11, E12).

**Cost per run.** Six trials × k=1 on flash at a one-hour multiplier: ≤ $4
worst case, ≈ $1 typical. At k=3: ≤ $12. Docker image builds are free of
API spend but not of time (600 s build budget each).

**Wall time.** Sequential worst case 6 h at k=1; `-n 2` on this Mac halves
it; expect 1-3 h for a k=1 run that mostly finishes early.

**Measures what the others cannot.** Pass against a public frontier row on
the same tasks, the same verifier, the same instruction suffix; economy per
pass on hard, externally authored work; whether Yi's harness survives eight
hours of a real environment (compaction, cache, the todo interception under
a long horizon).

**Cannot measure.** Persistence's `asked_twice` (one user message);
`answer_shape` by request class (every task is `change`); anything about the
Yi repository's own shape; comparability to the leaderboard proper (a
subset with a multiplier is a ledger row, never a leaderboard claim).

## (b) A Yi-owned task set in harbor's task format

**What it is.** `evals/fixtures/tasks/<id>/` grows harbor's layout
(`task.toml`, `instruction.md`, `environment/Dockerfile`, `tests/test.sh`
writing `/logs/verifier/reward.txt`, `solution/solve.sh`), so the same
directory runs under `harbor run -p evals/fixtures/tasks/<id>` and under
`evals/run.py`, which learns to read `instruction.md` and `tests/test.sh`
beside the `prompt.txt` / `reward.sh` pair it reads today. Tasks are
Yi-shaped requests: an enumerated five-item change, a diagnosis with a
planted bug that demands a regression test seen red, a refactor across
three files, one that must stop and block on the user. Small repositories
(a crate of a few hundred lines, a Python package), so an image builds in
seconds and a trial ends in minutes.

**Cost per run.** Cents. Row 0012 priced three fixture tasks at $0.002; a
ten-task set with real work is ≈ $0.10-0.50 on flash per k.

**Wall time.** Minutes per trial; a ten-task run in under an hour.

**Measures what the others cannot.** The persistence and rigor columns with
a verifier beside them: a task whose reward requires the regression test,
the fifth item of five, the block on the user. Cheap enough to run at k=5
and read variance. The `dryReward` tier stays offline and keeps the runner
honest (`evals/README.md`, "Task runner").

**Cannot measure.** Difficulty we did not author, so pass rates are
uninformative about the frontier; nothing about eight-hour horizons; the
same tasks will be seen by the prompt author, so they measure the doctrine's
mechanics, not its generality.

## (c) Hybrid: v4 subset for pass@k, Yi tasks and journeys for the other axes

**What it is.** (a) for the pass and economy axes at k=1 first and k=3 when
a row is worth defending; (b) plus `evals/journeys/ab.py` for persistence,
rigor and experience, all three scored by one stdlib script (`evals/axes.py`)
that reads a run directory of any of the three shapes and prints the per-trial
table and the ledger row. One extractor, one row shape, three sources.

**Cost per run.** (a) + (b): ≈ $1-4 for the v4 slice, ≈ $0.50 for the Yi
tasks, ≈ $0.10 for ten journeys on flash (the prompt-surface gate runs cost
about a cent each). A full hybrid pass stays under $5 on flash.

**Wall time.** Dominated by (a): 1-3 h. (b) and the journeys fit inside it
on the second concurrency slot.

**Measures.** Every column in `axes.md`, each from the source where it means
something. Cross-reads the others cannot: the same `intercept_count` on a
v4 trial and on a Yi task tells whether the todo interception scales with
horizon; the same `cache_miss_streak` on both tells whether a docker
environment changes the prefix (it should not).

**Cannot measure.** A leaderboard number. Frontier comparability beyond the
six tasks. Anything a person did not read: the columns locate, they do not
judge.

## Recommendation: (c), with (a) first

Take the hybrid. The reason is the question this plan exists to answer:
"measure coding performance, persistence, quality of the experience,
software engineering rigor while balancing speed, token use, and cost". (a)
alone answers the first and last of those and says nothing a person could
act on about the middle three. (b) alone measures a doctrine against tasks
its author wrote. (c) puts the external number and the internal instrument
on one row, and the marginal cost of (b) over (a) is under a dollar and one
stdlib script.

Within (c), run (a) first, because it is the run with four red preconditions
and two driver defects between it and a row (`yi-fit.md`); (b) and the
journeys can run today on the debug binary and are the cheaper place to
prove `evals/axes.py` before it prices anything.

## The first paid run against the caps

`tb21_cost.py --soft 20 --hard 25` (`evals/drivers/README.md`, "Caps") is
kept as the money gate, pointed at the directory harbor writes.

| step | spend | wall |
|---|---|---|
| oracle over the six tasks (`--agent oracle`, no key, no model): proves docker, harbor, the dataset digest and every verifier | $0 | ≈ 30-60 min, image builds |
| `--dry-run` of the Yi agent command (`cli/jobs.py:1250`) | $0 | seconds |
| one task, k=1, flash, one-hour multiplier: proves install, `--json` stream, sync, parse, the cap probe | ≤ $0.65, expect ≈ $0.10 | ≤ 1 h |
| the six tasks, k=1 | ≤ $4, expect ≈ $1 | 1-3 h at `-n 2` |
| **total for the first ledgered v4 row** | **≤ $5, expect ≈ $1.20** | **≈ half a day** |

Against the caps that is a quarter of the soft cap at worst. The soft cap
is not the binding constraint; the eight-hour agent timeout is, which is why
the multiplier is fixed at 0.125 (plan §8) and the row's `config-fp`
carries it.

The k=3 row that would make the pass@k column meaningful is ≤ $12 worst
case, ≈ $3-4 expected, and runs another day.
