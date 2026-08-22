#!/usr/bin/env python3
"""Every YI_* env var is a declared row; cap 40. Incident: jcode's 517 JCODE_* vars, 87%
undocumented."""
import json, re, sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, BASE, src_files, fail

declared = set(json.loads((BASE / "env_vars.json").read_text()))
found = set()
for f in src_files():
    found |= set(re.findall(r"YI_[A-Z_]+", f.read_text()))
errs = [f"undeclared env var {v} (add to baselines/env_vars.json)" for v in sorted(found - declared)]
if len(declared) > 40:
    errs.append(f"env surface {len(declared)} > hard cap 40")
fail(errs, f"env_surface ({len(found)} used / {len(declared)} declared)")
