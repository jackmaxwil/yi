#!/usr/bin/env python3
"""No committed file > 512 KB outside the allowlist."""
import sys, subprocess, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, BLOB_ALLOW, BLOB_CAP, fail

allow = set((ROOT / BLOB_ALLOW).read_text().split())
out = subprocess.run(["git", "ls-files", "-co", "--exclude-standard"], cwd=ROOT,
                     capture_output=True, text=True, check=True).stdout.split("\n")
errs = []
for rel in filter(None, out):
    p = ROOT / rel
    if rel not in allow and p.is_file() and p.stat().st_size > BLOB_CAP:
        errs.append(f"{rel}: {p.stat().st_size} bytes > {BLOB_CAP} (allowlist: {BLOB_ALLOW})")
fail(errs, "blob_size")
