#!/usr/bin/env python3
"""Zero panic budget from day 0 (not ratcheted down). Incident: jcode grandfathered 3,248
swallowed errors and 225k LOC over ceiling; both of its zero-start gates held."""
import json, re, sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, BASE, src_files, prod_lines, fail

budget = json.loads((BASE / "panic_budget.json").read_text())["budget"]
pat = re.compile(r"\.unwrap\(\)|\.expect\(|panic!\(|todo!\(|unimplemented!\(")
hits = []
for f in src_files():
    for i, line in enumerate(prod_lines(f), 1):
        if pat.search(line):
            hits.append(f"{f.relative_to(ROOT)}:{i}: {line.strip()}")
errs = hits if len(hits) > budget else []
if errs:
    errs.append(f"total {len(hits)} > budget {budget}")
fail(errs, f"panic ({len(hits)}/{budget})")
