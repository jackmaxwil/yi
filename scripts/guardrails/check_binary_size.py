#!/usr/bin/env python3
"""Dist binary size budget (13.6, D31: ratchets measure dist, never release)."""
import json, sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import BASE, DIST_BIN, fail

bin_path = DIST_BIN
if not bin_path.exists():
    fail([f"{bin_path} missing - run: cargo build --profile dist -p yi-cli"], "binary_size")
size = bin_path.stat().st_size
budget = json.loads((BASE / "binary_size_budget.json").read_text())["max_bytes"]
errs = [f"dist binary {size} > {budget} bytes"] if size > budget else []
fail(errs, f"binary_size ({size / 1048576:.2f} MiB / {budget / 1048576:.0f} MiB)")
