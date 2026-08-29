#!/usr/bin/env python3
"""Shrink-only per-crate src ceilings (D43 and its successors).
Incident: yi-tui's 10,000-line ceiling was prose only, so the crate reached 10,913
before anyone measured it."""
import json, sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, BASE, fail

budget = json.loads((BASE / "crate_size_budget.json").read_text())
sizes = {
    name: sum(len(f.read_text().splitlines()) for f in (ROOT / "crates" / name / "src").rglob("*.rs"))
    for name in budget
}
if "--update" in sys.argv:
    (BASE / "crate_size_budget.json").write_text(json.dumps(sizes, indent=2, sort_keys=True) + "\n")
    print("crate sizes " + ", ".join(f"{n} {budget[n]} -> {s}" for n, s in sorted(sizes.items())))
    sys.exit(0)

errs = [f"crates/{n}/src {s} lines > {budget[n]}" for n, s in sorted(sizes.items()) if s > budget[n]]
fail(errs, "crate_size (" + ", ".join(f"{n} {s}/{budget[n]}" for n, s in sorted(sizes.items())) + ")")
