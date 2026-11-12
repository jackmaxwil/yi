---
issue: Closes #2141
decision: Three guardrail gates announce a skip in a checkout that has no scripts/mirror and stay failures in a source checkout | the public mirror omits scripts/mirror and the gitleaks scan fixtures, so bridge.py, scripts/mirror/scan.py and check_scan.py selfchecks crashed there and a mirror contributor could not get just check green | delete the skip branches; the gates then fail in a mirror checkout again
---
A checkout of the public mirror now passes the guardrail gates that need files the mirror does not carry. `bridge.py --selfcheck`, `scripts/mirror/scan.py --selfcheck` and the scan selfcheck each print a `skip` row naming the missing input there. A source checkout has `scripts/mirror`, so every one still runs and a missing fixture is still a failure.
