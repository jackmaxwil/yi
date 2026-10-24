#!/usr/bin/env python3
"""Dist binary size budget (13.6, D31: ratchets measure dist, never release).
D249 hard cap: PR #436 hand-typed the baseline straight to 7 MiB in an empty-body
commit, and the ratchet then hid 865,664 bytes of growth across 20 PRs because
nothing checked the baseline itself. The 9 MiB cap below is checked independently of
the baseline so the baseline can never be edited past it, and the cap itself moves only
with a `decision:` line: it went 7, 8, 9 MiB, each time the baseline reached it."""
import json, os, platform, re, shutil, subprocess, sys, pathlib, tempfile
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import BASE, DIST_BIN, fail, fork, no_fork
import check_changes

HARD_CAP_BYTES = 9_437_184  # 9 MiB: D249 set 8, raised for the console workbench (#1354)
# Incident: 71 of 150 ratchet commits moved the baseline by 256 bytes or less; the linker grows
# the file a 16 KiB page at a time, so growth inside one page is alignment, not a ratchet.
PAGE = 16_384
CAP = re.compile(r"^HARD_CAP_BYTES = ([\d_]+)", re.M)
SELF = "scripts/guardrails/check_binary_size.py"


def baseline(system=None):
    """D70: the macOS file counts a Mach-O arm64 binary, so a Linux host is ratcheted on its own."""
    linux = (system or platform.system()) == "Linux"
    return BASE / ("binary_size_budget_linux.json" if linux else "binary_size_budget.json")


def problems(size, budget, hard_cap=HARD_CAP_BYTES, name="binary_size_budget.json"):
    errs = []
    if budget > hard_cap:
        errs.append(f"{name} max_bytes {budget} > hard cap {hard_cap} bytes (9 MiB)")
    if size > hard_cap:
        errs.append(f"dist binary {size} > hard cap {hard_cap} bytes (9 MiB)")
    elif size > budget + PAGE:
        errs.append(f"dist binary {size} > {budget} + {PAGE} bytes ({name})")
    return errs


def cap_errors(was, now, decisions):
    if was is not None and now > was and not decisions:
        return [f"HARD_CAP_BYTES {was} -> {now}: a raise needs a `decision:` line in this branch's change file"]
    return []


def fork_cap(base):
    found = CAP.search(check_changes.at(base, SELF) or "")
    return int(found.group(1).replace("_", "")) if found else None


def scratch_cases():
    """The cap check run in a scratch repo cut from main: a raise is red, a decision greens it."""
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="yi-binary-"))
    env = {k: v for k, v in os.environ.items() if k != "CARGO_TARGET_DIR"}
    run = lambda *a: subprocess.run(a, cwd=tmp, capture_output=True, text=True, env=env)
    git_ = lambda *a: run("git", "-c", "user.name=t", "-c", "user.email=t@t", *a)
    gate = tmp / SELF
    verdict = lambda: run(sys.executable, SELF)
    bad = []
    try:
        shutil.copytree(pathlib.Path(__file__).parent, gate.parent, ignore=shutil.ignore_patterns("baselines", "__pycache__"))
        (gate.parent / "baselines").mkdir()
        for name in ("binary_size_budget.json", "binary_size_budget_linux.json"):
            (gate.parent / "baselines" / name).write_text('{"max_bytes": 1000}\n')
        (tmp / "target/dist").mkdir(parents=True)
        (tmp / "target/dist/yi").write_bytes(b"\0" * 500)
        git_("init", "-q", "-b", "main")
        git_("add", "-A")
        git_("commit", "-q", "-m", "seed")
        git_("checkout", "-q", "-b", "topic")
        out = verdict()
        if out.returncode != 0:
            bad.append(f"an unmoved cap read red: {out.stdout.strip()}")
        gate.write_text(gate.read_text().replace("HARD_CAP_BYTES = 9_437_184", "HARD_CAP_BYTES = 10_485_760", 1))
        out = verdict()
        if out.returncode != 1 or "HARD_CAP_BYTES 9437184 -> 10485760" not in out.stdout:
            bad.append(f"a cap raised with no decision read {out.returncode}: {out.stdout.strip()}")
        (tmp / "docs/changes").mkdir(parents=True)
        (tmp / "docs/changes/2026-10-01-cap.md").write_text("---\ndecision: raise the cap | it grew | lower it\n---\nRaise.\n")
        out = verdict()
        if out.returncode != 0:
            bad.append(f"a cap raised with a decision read red: {out.stdout.strip()}")
    finally:
        shutil.rmtree(tmp)
    return bad


def selfcheck():
    """Each branch proven for its own reason, the way check_request_budget.py's flag does."""
    cap = 1_000_000
    # a baseline hand-edited above the cap fails even though no binary was measured over it
    errs = problems(500_000, 2_000_000, hard_cap=cap)
    assert errs == [f"binary_size_budget.json max_bytes 2000000 > hard cap {cap} bytes (9 MiB)"], errs
    # the cap catches an over-cap size even with a compliant baseline at the cap itself
    errs = problems(1_500_000, cap, hard_cap=cap)
    assert errs == [f"dist binary 1500000 > hard cap {cap} bytes (9 MiB)"], errs
    # under the cap, the ordinary per-PR ratchet still fires on its own
    errs = problems(900_000, 800_000, hard_cap=cap)
    assert errs == ["dist binary 900000 > 800000 + 16384 bytes (binary_size_budget.json)"], errs
    # a compliant size and baseline is clean
    assert problems(700_000, 800_000, hard_cap=cap) == []
    # growth inside one linker page passes, at the page itself too; one byte past it fails
    assert problems(800_016, 800_000, hard_cap=cap) == []
    assert problems(800_000 + PAGE, 800_000, hard_cap=cap) == []
    errs = problems(800_001 + PAGE, 800_000, hard_cap=cap, name="binary_size_budget_linux.json")
    assert errs == ["dist binary 816385 > 800000 + 16384 bytes (binary_size_budget_linux.json)"], errs
    # the shipped cap is 9 MiB: a baseline one byte past it fails, one at it does not
    assert HARD_CAP_BYTES == 9 * 1_048_576
    assert problems(0, HARD_CAP_BYTES + 1)[0].startswith("binary_size_budget.json max_bytes")
    assert problems(0, HARD_CAP_BYTES) == []
    assert baseline("Linux").name == "binary_size_budget_linux.json" and baseline("Darwin").name == "binary_size_budget.json"
    assert cap_errors(8, 9, []) and not cap_errors(8, 9, ["a | b | c"]) and not cap_errors(9, 9, []) and not cap_errors(None, 9, [])
    bad = scratch_cases()
    assert not bad, bad
    print("ok   binary_size selfcheck")


def main(argv):
    if "--selfcheck" in argv:
        selfcheck()
        return 0
    base = fork()
    if base is None:
        no_fork("binary_size")
    decisions = [d for _, fields in check_changes.pending(base) for d in fields["decision"]]
    errs = cap_errors(fork_cap(base), HARD_CAP_BYTES, decisions)
    if not DIST_BIN.exists():
        fail(errs + [f"{DIST_BIN} missing - run: scripts/build_dist.sh"], "binary_size")
    size = DIST_BIN.stat().st_size
    path = baseline()
    budget = json.loads(path.read_text())["max_bytes"]
    errs += problems(size, budget, name=path.name)
    fail(errs, f"binary_size ({size} bytes; {path.name} {budget} + {PAGE}, cap {HARD_CAP_BYTES})")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
