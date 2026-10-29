---
version: 0.798.0
issue: Closes #1745
---
Main goes red when change files wait unrecorded too long. `python3 scripts/record_changes.py --drift` exits 1 when more than 10 change files are unrecorded or the oldest landed more than 3 days ago, and each error names the count, the oldest file and the command that clears it (`python3 scripts/record_changes.py`, or `--backfill` while nothing was ever recorded). The postmerge job runs it as its last step on every push to main, whatever the record job did, so a record job that is switched off or failing shows on main within three days instead of four weeks. `--selfcheck` holds both checks at the cap and one past it, and fails when either is switched off.
