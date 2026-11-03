---
issue: Refs #1733
---

The planted gitleaks fixtures are fake keys the scanners have to catch, and GitHub's push
protection catches them too, so the mirror's push was refused with `GH013` and the public mirror
stopped publishing. `scripts/mirror/exclude.txt` now drops `scripts/scan/fixtures/gitleaks/`, and
`check_public_surface.py --selfcheck` asserts that every planted fixture in that directory is
excluded, so a line removed again fails the gate instead of the next push.

Semgrep kept a per-file time limit, at 300 seconds instead of 30: a loaded runner exceeded 30 on
one rule in `scripts/pr_autofix.py`, and `--timeout 0` answered that by removing the limit for
every file the guardrails scan. A `Timeout` entry in semgrep's report is now announced as a
`skip semgrep <path> (<rule> ran over 300s)` row naming the file and the rule it gave up on, and
every other error in the report still fails the scan. `check_scan.py --selfcheck` judges the
argv's `--timeout` and both directions of that split, and its `semgrep-timeout` mutant is refused
like the others.

The autofix pass drops its worker to a throwaway user, and on the guardrails runner the
interpreter was a shim under a root-only `HOME`: the job died on `PermissionError: [Errno 13]
Permission denied: '/root/.local/bin/python3.12'` before the worker started. The box now carries
a readable copy of the interpreter and runs the worker from it, and the selfcheck proves both
halves of that choice: a reachable interpreter runs where it is, and a root-only one is carried
in and left readable to the user it drops to.

Two edges of that carry are now held. An interpreter unreadable to the dropped-to user, and
outside the prefix the box can copy, is refused with a message naming it instead of being handed
to the worker, because a path that user cannot exec only moves the `PermissionError` one call
deeper. And widening the copied prefix's modes skips every symlink the copy carries, so no target
outside the box is chmod'd: before, a link in the runner's prefix to a root-only file made the
fixer widen that file in place.

`check_scan.py --selfcheck` also drives `semgrep()` itself now, over a faked report, so the
announced skip row is judged where the model and the CI log read it rather than one call short of
the print.
