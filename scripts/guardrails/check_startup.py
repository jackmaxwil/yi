#!/usr/bin/env python3
"""Startup budget via hyperfine (design §18.6). Fails loudly if hyperfine is missing (design §21:
a guardrail whose tooling is absent must not silently pass).

Scored on the *minimum* of the run, not the mean: startup is bounded below by real
work and only ever inflated by contention, so the fastest run is the honest cost of
the binary and the mean is a reading of how busy the machine was. `just guardrails`
depends on `build-dist`, so this gate runs right after an LTO build more often than
not; the mean read 6.5-7.4 ms there against 3.1-4.4 ms idle on the same binary, and
that is the number that blocked a push in a path `--version` never touches.

`--version` reaches no provider, session store or checkpointer, so a second entry times one
`yi ask` on faux. Every entry runs under a scratch HOME (kernel prewarm off) in an empty
scratch cwd under target/, never /tmp, whose whole tree the checkpointer would add, with git's
discovery stopped above it, so no lane is claimed in the checkout around it."""
import json, os, shutil, subprocess, sys, tempfile, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import BASE, DIST_BIN, fail


def scratch():
    target = DIST_BIN.parent.parent
    target.mkdir(parents=True, exist_ok=True)
    root = pathlib.Path(tempfile.mkdtemp(prefix="startup-", dir=target))
    (root / "home/.yi").mkdir(parents=True)
    (root / "home/.yi/config.json").write_text('{"kernel":{"prewarm":false}}')
    (root / "cwd").mkdir()
    return root


def errors(bin_path, budgets, runs=50, off="", env=None):
    errs, report = [], []
    root = scratch()
    try:
        # Incident: target/ sits inside the checkout, so each timed `yi ask` claimed a lane, a
        # `git worktree add` of the whole repo; the ceiling makes the cwd read as no repository.
        env = {**os.environ, **(env or {}), "HOME": str(root / "home"), "GIT_CEILING_DIRECTORIES": str(root)}
        if off == "home":
            env["HOME"] = os.environ.get("HOME", "")
        if off == "ceiling":
            env.pop("GIT_CEILING_DIRECTORIES")
        for args, max_ms in budgets.items():
            if off == args:
                continue
            argv = "--version" if off == "argv" and args != "yi --version" else args.removeprefix("yi").strip()
            # Incident: without --shell=none hyperfine subtracts a calibrated shell
            # startup, which drove D93's minimum to 0.00 ms and left the budget
            # unable to fail on any binary at all.
            out = root / "hyperfine.json"
            run = subprocess.run(["hyperfine", "-N", "--warmup", str(runs // 5), "--runs", str(runs),
                                  "--export-json", str(out), f"{bin_path} {argv}"],
                                 capture_output=True, text=True, cwd=root / "cwd", env=env)
            if run.returncode:
                errs.append(f"{args}: hyperfine exit {run.returncode}: {run.stderr.strip()[-300:]}")
                continue
            min_ms = json.loads(out.read_text())["results"][0]["min"] * 1000
            report.append(f"{args}: {min_ms:.2f} ms / {max_ms:.1f} ms")
            if min_ms > max_ms:
                errs.append(f"{args}: {min_ms:.2f} ms > {max_ms} ms")
    finally:
        shutil.rmtree(root, ignore_errors=True)
    return errs, report


STAND_IN = """#!/bin/sh
case "$HOME:$PWD" in */startup-*/home:*/startup-*/cwd) ;; *) echo "not in the scratch HOME and cwd" >&2; exit 3 ;; esac
[ "$GIT_CEILING_DIRECTORIES/cwd" = "$PWD" ] || { echo "the cwd can see a repository above it" >&2; exit 4; }
[ "$1" = "$SLOW" ] && sleep 0.02
echo ok
"""


def selfcheck(off=""):
    """A stand-in that sleeps 20 ms on one entry's first argument fails that entry, and only that
    one, against a 10 ms budget; it refuses to answer outside the scratch HOME and cwd, or below a visible repository."""
    budgets = {"yi --version": 10.0, "yi ask --model faux/faux-1 hi": 10.0}
    errs = []
    with tempfile.TemporaryDirectory() as tmp:
        stand_in = pathlib.Path(tmp) / "yi"
        stand_in.write_text(STAND_IN)
        stand_in.chmod(0o755)
        for slow, args in (("--version", "yi --version"), ("ask", "yi ask --model faux/faux-1 hi")):
            found, _ = errors(stand_in, budgets, runs=3, off=off, env={"SLOW": slow})
            if len(found) != 1 or not found[0].startswith(f"{args}: ") or " ms > 10.0 ms" not in found[0]:
                errs.append(f"slow {slow}: gate said {found}, not one over-budget line for {args!r}")
    return errs


if __name__ == "__main__":
    if shutil.which("hyperfine") is None:
        fail(["hyperfine not installed (brew install hyperfine)"], "startup")
    if "--selfcheck" in sys.argv:
        errs = selfcheck()
        for off in ("yi --version", "yi ask --model faux/faux-1 hi", "argv", "home", "ceiling"):
            if not selfcheck(off):
                errs.append(f"selfcheck passes with the {off} mutant, so it refutes nothing")
        fail(errs, "startup selfcheck")
        sys.exit(0)
    if not DIST_BIN.exists():
        fail([f"{DIST_BIN} missing - run: cargo build --profile dist -p yi-cli"], "startup")
    errs, report = errors(DIST_BIN, json.loads((BASE / "startup_ms_budget.json").read_text()))
    fail(errs, f"startup ({'; '.join(report)})")
