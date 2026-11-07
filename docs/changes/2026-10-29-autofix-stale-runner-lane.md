---
version: 0.986.0
decisions: D569
issue: Closes #1334
decision: A draft branch whose own copy of a workflow still carries a `runs-on` line the base changed is merged with its base after ten quiet minutes, not two hours (amends D344 for this case only) | review.yml moved to the llm runner on 2026-10-12 and ten branches that never merged main kept running their copy on ubuntu-latest, holding five of six general runners; a ready PR mid-landing must not be pushed to | delete stale_lane and the stale argument of decide in scripts/pr_autofix.py; D344 then applies unchanged
---
The autofixer now merges the base into a draft PR branch after ten quiet minutes, not two hours, when the base changed a workflow's `runs-on` line since the branch left it and the branch's own copy of that workflow still carries the old line (D541). On 2026-10-12 `review.yml` moved to the `llm` runner, ten branches that had not merged main kept running their own copy on `ubuntu-latest`, and those runs held five of the six general runners for nearly an hour. A ready PR, a workflow the base added or deleted, and a branch that already moved the line are left alone, and `autofix:hold` still keeps the fixer out.
