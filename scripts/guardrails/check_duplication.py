#!/usr/bin/env python3
"""15-line normalized-window duplication, production = 0. Incident: jcode's 7,779 duplicated
blocks; codex ships its patch grammar in four places."""
import sys, pathlib
from collections import defaultdict
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, src_files, prod_lines, fail

W = 15
windows = defaultdict(list)
for f in src_files():
    lines = [l.strip() for l in prod_lines(f) if l.strip()]
    for i in range(len(lines) - W + 1):
        windows[hash(tuple(lines[i:i + W]))].append(f"{f.relative_to(ROOT)}:{i + 1}")
errs = [f"duplicate 15-line window: {', '.join(locs[:4])}" for locs in windows.values() if len(locs) > 1]
fail(errs, "duplication")
