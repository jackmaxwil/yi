#!/usr/bin/env python3
"""Dependency counts, ratcheted (13.6): direct and transitive, default features."""
import json, subprocess, sys, tomllib, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, BASE, fail

budget = json.loads((BASE / "deps_budget.json").read_text())
direct = set()
for m in (ROOT / "crates").glob("*/Cargo.toml"):
    t = tomllib.loads(m.read_text())
    direct |= {d for d in t.get("dependencies", {}) if not d.startswith("yi-")}
out = subprocess.run(["cargo", "tree", "-e", "normal", "--workspace", "--prefix", "none"],
                     cwd=ROOT, capture_output=True, text=True, check=True).stdout
trans = {l.split()[0] for l in out.splitlines() if l.strip()} - {""}
trans = {c for c in trans if not c.startswith("yi-")}
errs = []
if len(direct) > budget["direct"]:
    errs.append(f"direct deps {len(direct)} > {budget['direct']}: {sorted(direct)}")
if len(trans) > budget["transitive"]:
    errs.append(f"transitive deps {len(trans)} > {budget['transitive']}")
fail(errs, f"deps ({len(direct)} direct / {len(trans)} transitive)")
