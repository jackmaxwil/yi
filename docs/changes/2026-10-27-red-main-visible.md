---
issue: Refs #1679
raise: tests +13
---
`pr merge` now waits on `gate (guardrails)`, `gate (lint)` and `gate (test)` whatever the branch protection rule lists, and names the head the gates were read on when it merges, so a head that moved is refused. The postmerge journeys lane is green again: the three terminal-pane journeys that had failed on every run since the pane landed are repaired, `closing_a_terminal_pane_ends_its_shell` no longer drops keys, `journeys` runs every test binary rather than stopping at the first red one, the evals and the live-ledger append run after a red journey, and each main commit now carries the PR gate's `test` lane as a status.
