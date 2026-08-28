#!/usr/bin/env python3
"""Request-prefix budget, ratcheted (design 9): the system block plus the tool
table are what every turn pays before the conversation starts. Measured by
`yi-runtime`'s request_budget test, which owns the fixture."""
import json, re, subprocess, sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, BASE, fail

budget = json.loads((BASE / "request_budget.json").read_text())
out = subprocess.run(
    ["cargo", "test", "-q", "-p", "yi-runtime", "--test", "request_budget",
     "report_the_prefix_size", "--", "--nocapture"],
    cwd=ROOT, capture_output=True, text=True)
if out.returncode != 0:
    fail([f"request_budget test failed:\n{out.stdout}{out.stderr}"], "request_budget")
found = re.search(r"REQUEST_PREFIX system=(\d+) tools=(\d+) total=(\d+)", out.stdout)
if found is None:
    fail(["request_budget test printed no REQUEST_PREFIX line"], "request_budget")
system, tools, total = (int(found.group(n)) for n in (1, 2, 3))

if "--update" in sys.argv:
    (BASE / "request_budget.json").write_text(
        json.dumps({"system": system, "tools": tools, "total": total}) + "\n")
    print(f"ok   request_budget updated to {total} bytes")
    sys.exit(0)

errs = [f"{name} prefix {got} > budget {budget[name]}"
        for name, got in (("system", system), ("tools", tools), ("total", total))
        if got > budget[name]]
fail(errs, f"request_budget ({total}/{budget['total']} bytes: system {system}, tools {tools})")
