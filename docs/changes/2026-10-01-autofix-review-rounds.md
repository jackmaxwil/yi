---
issue: Refs #976
decision: the autofix pass answers a round that blocked a PR's head, after conflicts and the same quiet wait, with a fresh `yi ask` that fixes each high and medium finding or declines it with the file:line that shows it wrong, pushes to the PR branch, and stops at two fixes in a row or any declined high (`autofix:failed`, the owner decides); `just pr fix`, `cmd_fix` and `YI_REVIEW_FIX` go, and every bot comment ends with a status line of models, calls, tokens, cost, time and the PR's and the day's bot spend (revises D320) | the owner: "Push straight to PR", "Stop and wait", "Leave those alone", and "I want each posted comment to have a bottom status line with those metrics"; on 2026-10-01 nine of sixteen open PRs sat on blocked rounds nobody answered | delete the findings kind from `pr_autofix.decide` and restore `cmd_fix`
---
Blocked review rounds are answered by the autofixer (Refs #976). Once a PR is quiet, the same
pass that resolves conflicts hands a blocked round's high and medium findings to a fresh `yi ask`.
It must fix each one or decline it with the file:line that shows it wrong, and prove a test finding with a test that fails without the
fix; a change that deletes a test or drops assertions is refused. The fix is pushed to the PR and
reviewed like any push. A declined high, or two fixes in a row that do not clear the review, sets
`autofix:failed` for the owner. Every review round, voided round and autofix comment now ends
with a status line (models, calls, turns, tokens in, cached and out, cost, time, the PR's and
the day's bot spend), its meta line carries the same numbers, and `just pr spend` totals them.
A voided round posts its error and spend instead of nothing.

Conflict and findings fixes run on three tiers, low (GLM 5.3 flash), medium (GPT 6.1 Sol) and
high (Opus 5.5), by points: three per high finding or conflicted file, one per medium finding or
extra conflict hunk, with cuts at 10 and 15 that put 61/30/9 of the last 44 fixable rounds on
the three tiers. A fix the review did not clear, or a failed attempt, moves the next one a tier
up. Each comment's meta line names the models it paid for, and `just pr spend` totals per model.
