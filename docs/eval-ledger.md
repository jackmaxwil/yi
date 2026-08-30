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
| 0001 | 2026-08-29 | fixtures@375d9b1 | openrouter/z-ai/glm-5.3-flash | afe35990a54e | 3/3 | k=1 | 8158/23616/61 | 0.0010 | 21.4s | 4 | 8002 | 0 | first real-model run of `evals/run.py`; exact cost $0.00098134, catalog cross-check (0.075/0.25/0.015 per M) agrees to the cent-fraction, 0% discrepancy. Not TB2.1: docker daemon down, harbor not installed, musl target absent, so the 6-task subset never ran. |
| 0002 | 2026-08-29 | orient-census@364-files | none (read-only census) | `orient_census` v1; corpus `~/.yi/sessions`, 364 files, 34 with `ext_record` | — | — | — | $0.00 | — | — | — | — | Measurement run for `P4` and `P13`, both **not ready**. Rows per key: route 34, turn 34, cache 34, orchestrate_attached 2 (both `prefilter`). Route labels: one_shot 31, complex 2, undecided 1. First-route × later-escalation: **0 of 34 sessions escalated after their first route**, so the P4 fit has zero positive labels. Route rows carrying every fit feature: **0 of 34** — the pre-widening rows persist only words/repo_dirty/named_paths, 3 of 9. `get_context` packets in the corpus: **0**, so P13 has no layer-utilisation data at all. Thresholds (proposed, in the script): P4 ≥200 route rows and ≥20 escalations; P13 ≥50 packets. No constant was moved: `-3`/`+4` bounds and `LAYER_CAP` stand. Read 0/34 escalations as absent evidence, not as proof the bounds are right. |
| 0003 | 2026-08-29 | orient-census@373-files | none (read-only census) | `orient_census` v2 + `extract.py` v2; corpus `~/.yi/sessions`, 373 files, 41 with `ext_record`, 10 with a tool call | — | — | — | $0.00 | — | — | — | — | Re-measurement of `P4` and `P13` after the `Features` widening and the extractor's read-only fix. **`P4` is not undersampled, it is degenerate.** 39 route rows (one_shot 36, complex 2, undecided 1), 5 of them carrying all nine features, **0 escalations**, and — the finding the row count hid — **one feature varies across the whole corpus**: `words`. `repo_dirty` is false and `named_paths` is zero in all 39, and `score`/`enums`/`imperatives`/`questions`/`and_count`/`fenced` are constant across the 5 full-feature rows. A logistic fit over one live column and one effective class fits nothing, so the census gained a third P4 gate: ≥5 varying features beside ≥200 rows and ≥20 escalations. **`P13` still has 0 `get_context` packets**, so per-layer caps stay unfitted and `LAYER_CAP: usize = 4_000` stands. What did change is the P13 **baseline**: `extract.py` screened the tool name only, so a `pwd && ls -la` opener counted as a mutation; screening bash arguments through the `builtins.rs` read-only vocabulary moves orientation from "9 of 364 sessions reached a mutation, 6 of 9 with zero read-only calls" to **7 of 10 working sessions, mean 2.0 (median 2, range 0–4) read-only calls and median 6,689 (mean 18,538) tokens before the first mutation**; 3 working sessions never mutated. `wins` moves 329/364 (90%) → **4/10 working sessions (40%)** on the honest denominator. No constant moved in this run either. |
| 0004 | 2026-08-29 | fixtures@375d9b1 | openrouter/z-ai/glm-5.3-flash | afe35990a54e | 2/2 | k=1 | 7973/7872/29 | 0.0007233 | 11.3s | 2 | 7923 | 0 | **T2 baseline, rep 1 of 2**, the two easiest tasks that passed in run 0001 (`answer-echo`, `clean-workspace` — both single-turn; `edit-file` is the only one needing a tool call). Cold provider cache: `answer-echo` paid 7922 input at $0.075/M, `clean-workspace` then read the identical 7872-token prefix at $0.015/M. Catalog cross-check exact to the reported cent-fraction, 0% discrepancy. |
| 0005 | 2026-08-29 | fixtures@375d9b1 | openrouter/z-ai/glm-5.3-flash | afe35990a54e | 2/2 | k=1 | 101/15744/17 | 0.0002480 | 10.3s | 2 | 7923 | 0 | **T2 baseline, rep 2 of 2 — the k=2 variance replay.** Identical config to 0004. Pass did not flip (2/2 both reps) and per-task context is bit-stable (7922 / 7923 in both), so neither task is excluded from campaign conclusions. Cost is **6x lower than 0004 on the same config**, entirely from provider cache state: rep 1 wrote the prefix, rep 2 read it. Read cost deltas between rows only at equal cache warmth; `peak-ctx` is the order-independent B-axis number. |
| 0006 | 2026-08-29 | fixtures@375d9b1 | openrouter/z-ai/glm-5.3-flash | fcf1934b9bad | 2/2 | k=1 | 4373/4288/23 | 0.0003980 | 9.9s | 2 | 4331 | 0 | **T2 lever L1 — context policy: global skills catalog off** (`HOME` pointed at an empty dir, so `skills::discover_split` finds no global root; no code and no prompt asset changed). Same tasks, same binary, cold cache like 0004. Context **7922 → 4330 tokens per turn, −45.3%**; cold-cache per-task cost **$0.00059615 → $0.00032825, −44.9%**. Pass held 2/2. |
| 0007 | 2026-08-29 | fixtures@375d9b1 | openrouter/z-ai/glm-5.3-flash | fcf1934b9bad | 2/2 | k=1 | 85/8576/17 | 0.0001393 | 9.9s | 2 | 4331 | 0 | **L1 variance replay (k=2 on the variant).** Context again bit-stable at 4330 / 4331, pass 2/2. Warm-cache per-task cost $0.000127155 → $0.00006979, **−45.1%** — the warm and cold ratios agree to 0.2 points, so the −45% is the lever, not the cache. |
| 0008 | 2026-08-29 | fixtures@375d9b1 | openrouter/z-ai/glm-5.3-flash | afe35990a54e | 3/3 | k=1 | 285/31488/68 | 0.0005107 | 17.3s | 4 | 8001 | 0 | **Baseline full-suite control**, same binary and same day as 0009 — run 0001 is the older baseline and was measured on an earlier build, so the exit gate is judged against this row, not against 0001. |
| 0009 | 2026-08-29 | fixtures@375d9b1 | openrouter/z-ai/glm-5.3-flash | fcf1934b9bad | 3/3 | k=1 | 253/17152/77 | 0.0002955 | 20.4s | 4 | 4409 | 0 | **T2 exit gate: L1 over the full suite. Regressions 0.** 3/3 including `edit-file`, the only task that calls a tool, at the same 4 agent steps as the control. Against 0008 at equal cache warmth: peak-ctx **8001 → 4409 (−44.9%)**, suite cost **−42.1%**, pass unchanged. What the suite cannot show: no fixture task needs a skill, so this measures the catalog's price and not what it buys — the lever is a measured candidate, not a promotion. Headroom source, measured at $0 before the runs: the global catalog is 58 skills / 21,336 raw bytes fitted into `SourceBudgets::skills_meta` = 16,384 B, so it **saturates its budget** (25 skills' descriptions already truncated away) and is ~4,096 of the baseline's ~7,922 context tokens — 52% of the prefix, paid every turn. |
