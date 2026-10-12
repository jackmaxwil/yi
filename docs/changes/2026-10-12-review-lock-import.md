---
issue: Refs #1210
---
The review bot can take its per-PR round lock again. A fixer commit (b9db1d02) dropped `import fcntl` from `scripts/pr_review.py` while `held` still calls `fcntl.flock`, so every round on main died with `NameError` before reading a PR. The selfcheck now takes the lock once, so a missing import fails the guardrails rather than the review job.
