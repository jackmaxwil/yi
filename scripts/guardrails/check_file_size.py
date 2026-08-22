#!/usr/bin/env python3
"""File ceiling 1,200 lines, src/ only (D31: codex core/ is 67% test LOC — an unscoped ratchet
fires forever). Function ceiling (150, syn-based) lands with the first non-trivial .rs."""
import sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, src_files, fail

errs = []
for f in src_files():
    n = len(f.read_text().splitlines())
    if n > 1200:
        errs.append(f"{f.relative_to(ROOT)}: {n} lines > 1200")
fail(errs, "file_size")
