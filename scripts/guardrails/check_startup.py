#!/usr/bin/env python3
"""Startup budget via hyperfine (design §18.6). Fails loudly if hyperfine is missing (design §21:
a guardrail whose tooling is absent must not silently pass).

Scored on the *minimum* of the run, not the mean: startup is bounded below by real
work and only ever inflated by contention, so the fastest run is the honest cost of
the binary and the mean is a reading of how busy the machine was. `just guardrails`
depends on `build-dist`, so this gate runs right after an LTO build more often than
not; the mean read 6.5-7.4 ms there against 3.1-4.4 ms idle on the same binary, and
that is the number that blocked a push in a path `--version` never touches."""
import json, shutil, subprocess, sys, tempfile, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import BASE, DIST_BIN, fail

if shutil.which("hyperfine") is None:
    fail(["hyperfine not installed (brew install hyperfine)"], "startup")
bin_path = DIST_BIN
if not bin_path.exists():
    fail([f"{bin_path} missing - run: cargo build --profile dist -p yi-cli"], "startup")
budgets = json.loads((BASE / "startup_ms_budget.json").read_text())
errs = []
report = []
with tempfile.NamedTemporaryFile(suffix=".json") as tmp:
    for args, max_ms in budgets.items():
        cmd = str(bin_path) + args.removeprefix("yi")
        # Incident: without --shell=none hyperfine subtracts a calibrated shell
        # startup, which drove D93's minimum to 0.00 ms and left the budget
        # unable to fail on any binary at all.
        subprocess.run(["hyperfine", "-N", "--warmup", "10", "--runs", "50",
                        "--export-json", tmp.name, cmd],
                       capture_output=True, check=True)
        min_ms = json.load(open(tmp.name))["results"][0]["min"] * 1000
        report.append(f"{args}: {min_ms:.2f} ms / {max_ms:.1f} ms")
        if min_ms > max_ms:
            errs.append(f"{args}: {min_ms:.2f} ms > {max_ms} ms")
fail(errs, f"startup ({'; '.join(report)})")
