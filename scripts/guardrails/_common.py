import json, pathlib, subprocess, sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
# Net src lines a version may add unpriced (040). Here rather than in check_growth
# because check_pr_metadata prices the same band on a PR's own diff, and two
# copies of a budget is two budgets.
FREE_BAND = 150
BASE = ROOT / "scripts/guardrails/baselines"

def src_files():
    return sorted((ROOT / "crates").glob("*/src/**/*.rs"))

def prod_lines(path):
    """Lines before a #[cfg(test)] marker; inline test mods sit at file bottom by convention."""
    out = []
    for line in path.read_text().splitlines():
        if line.strip().startswith("#[cfg(test)]"):
            break
        out.append(line)
    return out

def fail(msgs, name):
    if msgs:
        print(f"FAIL {name}")
        for m in msgs:
            print(f"  {m}")
        sys.exit(1)
    print(f"ok   {name}")
