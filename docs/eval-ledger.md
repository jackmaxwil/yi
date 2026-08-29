# Eval ledger

One row per task-eval run. A run that is not in this table did not happen:
optimization claims cite a run-id, or they are vibes. Rows are appended, never
edited — a re-run is a new run-id. Columns are additive only; a new metric is a
new column on the right, so older rows stay readable.

Rows are written from the harness output after the run finishes, before any
commit cites them (plan §7, §8). The `Opt-Run:` commit trailer carries the same
zero-padded run-id.

## Columns

| column | meaning |
|---|---|
| run-id | zero-padded counter, shared with the `Opt-Run:` trailer |
| date | UTC date the run finished |
| suite@rev | dataset and its pinned revision, e.g. `tb21@a3f1c2d` |
| model | `provider/model` plus reasoning effort |
| config-fp | `yi_usage.config_fingerprint` — adapter version, model, mode, suite |
| pass | passed / attempted tasks (A axis) |
| pass@k | attempts per task and the resulting pass@k (A axis) |
| tokens | input / cached / output (B axis; input includes cache reads) |
| cost | self-reported USD from `usage.cost.total` (B axis) |
| wall | harness-measured wall clock per task (B axis) |
| turns | assistant steps, pier `n_agent_steps` (D/G axes) |
| peak-ctx | pier `peak_context_tokens` (F axis) |
| compactions | pier `summarization_count` (F axis) |
| notes | what was being tested; regressions; anything that makes the row unfair |

## Runs

| run-id | date | suite@rev | model | config-fp | pass | pass@k | tokens | cost | wall | turns | peak-ctx | compactions | notes |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
