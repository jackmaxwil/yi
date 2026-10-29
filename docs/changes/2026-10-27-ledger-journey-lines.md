---
version: 0.786.0
issue: Closes #1677
---
The feature ledger's journey tests move out of the 2-4 KB note cells into docs/ledger-journeys.txt, one `feature | test` line each, merged by git's union driver. Two PRs adding different journey tests to the same feature now append two lines and merge without a conflict instead of colliding on one row. A new guardrail, check_ledger_journeys.py, makes every ledger row name at least one journey line and refuses a line for a row that does not exist; its selfcheck merges two real branches to prove the union rule holds.
