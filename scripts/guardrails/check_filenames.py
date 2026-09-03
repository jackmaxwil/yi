#!/usr/bin/env python3
"""Bisection is not decomposition. Incident: part_01.rs..part_NN.rs file splits."""
import re, sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, src_files, fail

pat = re.compile(r"(part_\d+|_\d\d)\.rs$")
errs = [str(f.relative_to(ROOT)) for f in src_files() if pat.search(f.name)]
fail(errs, "filenames")
