---
version: 0.1029.0
decisions: D586
issue: Closes #2141
cut: none scripts/guardrails/check_scan.py
shares: #2135 the imported Max drafts carry the same mirror skip hunks; this branch is their split-out replacement
shares: #2136 the imported Max drafts carry the same mirror skip hunks; this branch is their split-out replacement
shares: #2137 the imported Max drafts carry the same mirror skip hunks; this branch is their split-out replacement
decision: Five guardrail gates announce a skip in a checkout that has no scripts/mirror and stay failures in a source checkout | the public mirror omits scripts/mirror and the gitleaks scan fixtures, so bridge.py, scripts/mirror/scan.py, check_scan.py, check_changes.py and record_changes.py selfchecks failed there (the last two pin source commits the mirror's main lacks) and a mirror contributor could not get just check green | delete the skip branches; the gates then fail in a mirror checkout again
---
A checkout of the public mirror now passes the guardrail gates that need files the mirror does not carry. `bridge.py`, `scripts/mirror/scan.py`, the scan, change-history and record-history selfchecks each print a `skip` row naming the missing input there. A source checkout has `scripts/mirror`, so every one still runs and a missing fixture is still a failure. No skip fires when `CI` is set, so a forge job that lost `scripts/mirror` fails instead of passing.
