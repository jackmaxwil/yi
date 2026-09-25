#!/usr/bin/env python3
"""Dist binary size budget (13.6, D31: ratchets measure dist, never release).
D244 hard cap: PR #436 hand-typed the baseline straight to 7 MiB in an empty-body
commit, and the ratchet then hid 865,648 bytes of growth across 20 PRs because
nothing checked the baseline itself. The cap below is checked independently of
baselines/binary_size_budget.json so the baseline can never be edited past it."""
import json, sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import BASE, DIST_BIN, fail

HARD_CAP_BYTES = 7_340_032  # 7 MiB, D244


def problems(size, budget, hard_cap=HARD_CAP_BYTES):
    errs = []
    if budget > hard_cap:
        errs.append(f"binary_size_budget.json max_bytes {budget} > hard cap {hard_cap} bytes (7 MiB, D244)")
    if size > hard_cap:
        errs.append(f"dist binary {size} > hard cap {hard_cap} bytes (7 MiB, D244)")
    elif size > budget:
        errs.append(f"dist binary {size} > {budget} bytes")
    return errs


def selfcheck():
    """Each branch proven for its own reason, the way check_request_budget.py's flag does."""
    cap = 1_000_000
    # a baseline hand-edited above the cap fails even though no binary was measured over it
    errs = problems(500_000, 2_000_000, hard_cap=cap)
    assert errs == [f"binary_size_budget.json max_bytes 2000000 > hard cap {cap} bytes (7 MiB, D244)"], errs
    # the cap catches an over-cap size even with a compliant baseline at the cap itself
    errs = problems(1_500_000, cap, hard_cap=cap)
    assert errs == [f"dist binary 1500000 > hard cap {cap} bytes (7 MiB, D244)"], errs
    # under the cap, the ordinary per-PR ratchet still fires on its own
    errs = problems(900_000, 800_000, hard_cap=cap)
    assert errs == ["dist binary 900000 > 800000 bytes"], errs
    # a compliant size and baseline is clean
    assert problems(700_000, 800_000, hard_cap=cap) == []
    print("ok   binary_size selfcheck")


def main(argv):
    if "--selfcheck" in argv:
        selfcheck()
        return 0
    bin_path = DIST_BIN
    if not bin_path.exists():
        fail([f"{bin_path} missing - run: cargo build --profile dist -p yi-cli"], "binary_size")
    size = bin_path.stat().st_size
    budget = json.loads((BASE / "binary_size_budget.json").read_text())["max_bytes"]
    errs = problems(size, budget)
    fail(errs, f"binary_size ({size / 1048576:.2f} MiB / {budget / 1048576:.0f} MiB)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
