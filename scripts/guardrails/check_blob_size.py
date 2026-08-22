#!/usr/bin/env python3
"""No committed file > 512 KB outside the allowlist (codex blob-size-policy.yml)."""
import sys, subprocess, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, BASE, fail

allow = set((BASE / "blob_allowlist.txt").read_text().split())
out = subprocess.run(["git", "ls-files", "-co", "--exclude-standard"], cwd=ROOT,
                     capture_output=True, text=True, check=True).stdout.split("\n")
errs = []
for rel in filter(None, out):
    p = ROOT / rel
    if rel not in allow and p.is_file() and p.stat().st_size > 512_000:
        errs.append(f"{rel}: {p.stat().st_size} bytes > 512000 (allowlist: baselines/blob_allowlist.txt)")
fail(errs, "blob_size")
