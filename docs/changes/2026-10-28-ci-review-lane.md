---
issue: Closes #1310
---
The runner map in `docs/FORGE.md` names the `llm` lane the review job runs on and the two rules that keep the gate's runners free of bot work: a pass that finds a newer autofix pass queued exits before it checks out or builds, and a round budgets its calls from the job's first step, stamped before the checkout, so a slow build cannot push it past the two-hour limit. `scripts/guardrails/check_workflows.py` now refuses a `review.yml` job on a lane any `pr.yml` gate job runs on, the regression that put gates behind model waits for up to four hours on 2026-10-12; its `--selfcheck` fails when that rule is switched off.
