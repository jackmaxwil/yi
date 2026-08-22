#!/usr/bin/env python3
"""Startup budget via hyperfine (13.6). Fails loudly if hyperfine is missing (design 9:
a guardrail whose tooling is absent must not silently pass)."""
import json, shutil, subprocess, sys, tempfile, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, BASE, fail

if shutil.which("hyperfine") is None:
    fail(["hyperfine not installed (brew install hyperfine)"], "startup")
bin_path = ROOT / "target/dist/yi"
if not bin_path.exists():
    fail([f"{bin_path} missing - run: cargo build --profile dist -p yi-cli"], "startup")
budgets = json.loads((BASE / "startup_ms_budget.json").read_text())
errs = []
report = []
with tempfile.NamedTemporaryFile(suffix=".json") as tmp:
    for args, max_ms in budgets.items():
        cmd = str(bin_path) + args.removeprefix("yi")
        subprocess.run(["hyperfine", "--warmup", "10", "--runs", "50", "--export-json", tmp.name, cmd],
                       capture_output=True, check=True)
        mean_ms = json.load(open(tmp.name))["results"][0]["mean"] * 1000
        report.append(f"{args}: {mean_ms:.2f} ms")
        if mean_ms > max_ms:
            errs.append(f"{args}: {mean_ms:.2f} ms > {max_ms} ms")
fail(errs, f"startup ({'; '.join(report)})")
