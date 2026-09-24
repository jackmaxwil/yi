#!/usr/bin/env python3
"""Behavior baseline, shrink-only (D76): each faux cassette under
crates/runtime/tests/fixtures/behavior replays to one BEHAVIOR verdict line and
the baseline locks it. A locked pass may never go red; a new red case enters
only through --update, in its own commit. The harness owns the replay, this
owns the ratchet — same split as request_budget."""
import json, re, subprocess, sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, BASE, fail

f = BASE / "behavior_baseline.json"
# --workspace, not -p: features then unify as in the test lane, so both reuse one build of
# the stack; -p yi-runtime compiled a second copy of 67 crates and relinked every binary.
out = subprocess.run(
    ["cargo", "test", "-q", "--workspace", "--test", "behavior", "--", "--nocapture"],
    cwd=ROOT, capture_output=True, text=True)
if out.returncode != 0:
    fail([f"behavior harness failed:\n{out.stdout}{out.stderr}"], "behavior")
# Incident: an unanchored parse read a cassette's own assertion text as a verdict,
# so a case printed `state=fail` and the gate scored it pass. Verdicts are whole
# lines, one per case; anything else is the harness lying to its own ratchet.
found = {}
for case, state in re.findall(r"^BEHAVIOR case=(\S+) state=(pass|fail)", out.stdout, re.M):
    if case in found:
        fail([f"{case}: two verdict lines in one run"], "behavior")
    found[case] = state
if not found:
    fail(["behavior harness printed no BEHAVIOR lines"], "behavior")

locked = json.loads(f.read_text())["cases"] if f.exists() else None

if "--update" in sys.argv:
    baked = [c for c, state in (locked or {}).items()
             if state == "pass" and found.get(c) == "fail"]
    if baked:
        fail([f"refusing to bake a regression: {c} was pass" for c in baked], "behavior")
    f.write_text(json.dumps({"cases": dict(sorted(found.items()))}, indent=2) + "\n")
    failing = sum(1 for state in found.values() if state == "fail")
    print(f"ok   behavior updated to {len(found)} cases, {failing} failing")
    sys.exit(0)

if locked is None:
    fail([f"no behavior baseline at {f.relative_to(ROOT)}; run --update in its own commit"],
         "behavior")

errs = []
for case, state in sorted(locked.items()):
    if case not in found:
        errs.append(f"{case}: cassette missing from the run; removal is a reviewed --update")
    elif state == "pass" and found[case] == "fail":
        errs.append(f"regression: case {case} was pass")
    elif state == "fail" and found[case] == "pass":
        errs.append(f"case {case} now passes — ratchet it with --update in its own commit")
for case in sorted(found):
    if case not in locked:
        errs.append(f"{case}: new case not locked; --update in its own commit")
failing = sum(1 for state in locked.values() if state == "fail")
fail(errs, f"behavior ({len(locked)} cases, {failing} failing)")
