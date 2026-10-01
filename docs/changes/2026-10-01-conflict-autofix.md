---
issue: Refs #976
decision: a 15-minute pass in a Forgejo runner merges the base into each same-repo PR that conflicts with it and has been quiet two hours or carries the `autofix` label, resolves what git cannot with a fresh `yi ask --auto` (GLM 5.3 flash, Opus 5.5 high from three conflicted files) in a token-free clone, and pushes to the PR branch as yi-bot; four labels (`autofix`, `autofix:hold`, `autofix:working`, `autofix:failed`) are its controls and state, and its comments are the ledger for a $8-per-PR and $25-a-day cap (revises D320's trigger and scope) | the owner: "build the conflict autofixer. include a full tagging system for robust usage and management", "Everything to the PR", "Next 15-min pass" | delete autofix.yml and scripts/pr_autofix.py
---
Conflicts with the base are fixed by a bot (Refs #976). Every 15 minutes, oldest PR first, the
autofixer merges the base into a same-repo PR that conflicts with it and has been quiet for two
hours, or at once when the PR carries the `autofix` label. A fresh `yi ask` resolves the files
git could not, in a clone with no forge token, then the hooks judge the commit and the fixer
pushes it to the PR branch as yi-bot and comments with what it did and what it cost.
`autofix:hold` keeps it out, `autofix:working` shows a fix in flight, and `autofix:failed` stops
it, with the reason in its comment, until a person removes the label. `just pr autofix [N]` runs
the same pass by hand.
