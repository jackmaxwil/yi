#!/usr/bin/env python3
"""Glob hygiene = 0, not budgeted. Incident: 63% of jcode glued into one namespace by
pub use ...::* chains; the crate split removed zero imports."""
import re, sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, src_files, prod_lines, fail

pat = re.compile(r"^\s*(pub\s+use\s+.*::\*|use\s+super::\*)")
errs = []
for f in src_files():
    for i, line in enumerate(prod_lines(f), 1):
        if pat.match(line):
            errs.append(f"{f.relative_to(ROOT)}:{i}: {line.strip()}")
fail(errs, "glob_reexport")
