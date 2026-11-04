---
version: 0.917.0
issue: Closes #1512
---
`just commit <subject> [<path>...]` stages the paths it is given by name instead of running `git add -A`, the one command `.ruler/090-workflow.md` forbids in a shared tree. With no path it commits what is already staged; with nothing staged it refuses and prints the dirty paths so the caller names them. The ratchet commit it makes first still takes only the baselines. With paths named, the commit itself is limited to them, so a file another session staged stays staged. `forge_pr.py --selfcheck` requires both: an unnamed untracked file stays out, and an unrelated staged file is not committed with the named one. The autofixer no longer builds yi or refreshes the OpenRouter catalog on every 15-minute pass: `.forgejo/workflows/autofix.yml` drops both steps and `pr_review.yi_bin` builds and refreshes once at the first model call (its own selfcheck pins that). The workflow half is a step deletion with no test of its own (Refs #1310).
