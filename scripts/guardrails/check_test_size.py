#!/usr/bin/env python3
"""Test LOC has its own ratchet (D31); growth needs --update."""
import json, sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, BASE, fail

f = BASE / "test_size_budget.json"
budget = json.loads(f.read_text())["budget_lines"]
total = sum(len(p.read_text().splitlines()) for p in (ROOT / "crates").glob("*/tests/**/*.rs"))
if "--update" in sys.argv:
    f.write_text(json.dumps({"budget_lines": total}) + "\n")
    print(f"ok   test_size (baseline updated to {total})")
    sys.exit(0)
errs = [f"test LOC {total} > budget {budget} (rerun with --update to record intentional growth)"] if total > budget else []
fail(errs, f"test_size ({total}/{budget})")
