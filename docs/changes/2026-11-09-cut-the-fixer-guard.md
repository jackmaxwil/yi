---
issue: Closes #2131
cut: none scripts/fixer_guard.py
cut: none scripts/guardrails/fixtures/fixer_guard/pr1829.log
cut: none scripts/guardrails/fixtures/fixer_guard/pr2062.log
cut: none scripts/pr_autofix.py
decision: the autofixer pushes what its fixer wrote with no gate that guesses the author's intent from the PR's own history: `scripts/fixer_guard.py` is cut, and `scripts/guardrails/check_intent.py` (a recorded `cut:` or `keep:` line, read on every branch by any author) is the only gate on a deliberate cut | the guard read "a commit deleted five or more lines an earlier commit of the same PR added" as a reversal, so on a young or imported PR every review-driven dedupe tripped it; on PR 2130 the fixer spent $3.38 (Opus 5.5, 63 turns) on findings that asked for exactly those edits and nothing was pushed, because the check ran only after the spend and a finding was deliberately no exemption, so removing `autofix:hold` retried into the same refusal | restore `scripts/fixer_guard.py` and its two incident logs from the commit before this one, put back the `scan` call before `push_head` and the `reversed` verdict in `scripts/pr_autofix.py`, and the selfcheck line in `scripts/guardrails/check_guardrails.sh`
---

The autofixer no longer refuses a push because it deletes lines the pull request added (Closes #2131). The fixer guard did that after the fixer had already been paid, and it could not tell a dedupe the review asked for from the cut of a README block the owner made on purpose. On PR 2130 it discarded a $3.38 fix this way. A deliberate cut or keep is protected by recording it in a change file, which `check_intent.py` holds on every branch whoever wrote it; the guard's two incident logs, #2062's README and #1829's upstream pin, were cases that record would have caught. The `autofix:hold` label stays as the owner's own stop.
